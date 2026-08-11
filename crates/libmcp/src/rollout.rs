//! Optional failure-atomic release selection and live host handoff.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

#[cfg(unix)]
use std::os::unix::{
    fs::{MetadataExt as _, PermissionsExt as _},
    net::{UnixListener, UnixStream},
};
#[cfg(unix)]
use std::{
    io::Write as _,
    process::{Child, Command},
    thread,
};

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use sha2::{Digest as _, Sha256};

/// Absolute path to the selected release pointer in managed processes.
pub const LIBMCP_RELEASE_CHANNEL_ENV: &str = "LIBMCP_RELEASE_CHANNEL";
/// Binary digest of the release running in a managed process.
pub const LIBMCP_RELEASE_GENERATION_ENV: &str = "LIBMCP_RELEASE_GENERATION";
/// Private successor-readiness socket used only during a two-phase handoff.
pub const LIBMCP_HANDOFF_SOCKET_ENV: &str = "LIBMCP_HANDOFF_SOCKET";

const RELEASE_SCHEMA: u32 = 1;
const POINTER_MAX_BYTES: usize = 8 * 1024;
const MANIFEST_MAX_BYTES: usize = 64 * 1024;
const SUCCESSOR_SETTLE_TIME: Duration = Duration::from_millis(200);
#[cfg(unix)]
const SUCCESSOR_GATE_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(unix)]
const READY: u8 = b'R';
#[cfg(unix)]
const ACTIVATE: u8 = b'A';
#[cfg(unix)]
const LIVE: u8 = b'L';

/// SHA-256 identity of one immutable release or one recorded build input.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ReleaseId(String);

impl ReleaseId {
    /// Refines one lowercase hexadecimal SHA-256 digest.
    pub fn try_new(value: impl Into<String>) -> io::Result<Self> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(invalid_data(
                "release id must be one lowercase hexadecimal SHA-256 digest",
            ));
        }
        Ok(Self(value))
    }

    /// Returns the digest text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Hashes one file into its immutable release identity.
    pub fn digest_file(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let mut hasher = Sha256::new();
        let _bytes = io::copy(&mut file, &mut hasher)?;
        release_id_from_digest(hasher.finalize().as_slice())
    }

    /// Hashes canonical bytes into an immutable identity.
    pub fn digest_bytes(bytes: &[u8]) -> io::Result<Self> {
        release_id_from_digest(Sha256::digest(bytes).as_slice())
    }
}

impl<'de> Deserialize<'de> for ReleaseId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_new(value).map_err(D::Error::custom)
    }
}

impl std::fmt::Display for ReleaseId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// State-format envelope used to adjudicate promotion and rollback.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum StateCompatibility {
    /// The server owns no state whose representation constrains a release.
    Stateless,
    /// The server reads explicit epochs and writes exactly one epoch.
    Versioned {
        /// State epochs accepted by this release.
        readable: BTreeSet<u64>,
        /// State epoch emitted by this release.
        writable: u64,
    },
}

impl StateCompatibility {
    /// Constructs a versioned contract that can read its own writes.
    pub fn versioned(readable: impl IntoIterator<Item = u64>, writable: u64) -> io::Result<Self> {
        let readable = readable.into_iter().collect::<BTreeSet<_>>();
        if !readable.contains(&writable) {
            return Err(invalid_data(
                "a release must accept the state epoch it writes",
            ));
        }
        Ok(Self::Versioned { readable, writable })
    }

    /// Returns whether this release can follow `incumbent` without a destructive migration.
    #[must_use]
    pub fn accepts(&self, incumbent: &Self) -> bool {
        match (self, incumbent) {
            (Self::Stateless, Self::Stateless) => true,
            (
                Self::Versioned { readable, .. },
                Self::Versioned {
                    writable: incumbent,
                    ..
                },
            ) => readable.contains(incumbent),
            (Self::Stateless, Self::Versioned { .. })
            | (Self::Versioned { .. }, Self::Stateless) => false,
        }
    }
}

/// Exact build inputs recorded beside one immutable release.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReleaseProvenance {
    repository: PathBuf,
    commit: String,
    cargo_lock_sha256: ReleaseId,
    rustc: String,
    built_at: String,
}

impl ReleaseProvenance {
    /// Constructs validated local build provenance.
    pub fn try_new(
        repository: PathBuf,
        commit: impl Into<String>,
        cargo_lock_sha256: ReleaseId,
        rustc: impl Into<String>,
        built_at: impl Into<String>,
    ) -> io::Result<Self> {
        let commit = commit.into();
        let rustc = rustc.into();
        let built_at = built_at.into();
        if !repository.is_absolute() {
            return Err(invalid_data("release repository path must be absolute"));
        }
        if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(invalid_data("release commit must be a full Git object id"));
        }
        if rustc.is_empty() || built_at.is_empty() {
            return Err(invalid_data(
                "release toolchain and build timestamp must be non-empty",
            ));
        }
        Ok(Self {
            repository,
            commit: commit.to_ascii_lowercase(),
            cargo_lock_sha256,
            rustc,
            built_at,
        })
    }

    /// Returns the source repository.
    #[must_use]
    pub fn repository(&self) -> &Path {
        &self.repository
    }

    /// Returns the exact source commit.
    #[must_use]
    pub fn commit(&self) -> &str {
        &self.commit
    }

    /// Returns the Cargo lockfile digest.
    #[must_use]
    pub const fn cargo_lock_sha256(&self) -> &ReleaseId {
        &self.cargo_lock_sha256
    }

    /// Returns the compiler identity.
    #[must_use]
    pub fn rustc(&self) -> &str {
        &self.rustc
    }

    /// Returns the build timestamp.
    #[must_use]
    pub fn built_at(&self) -> &str {
        &self.built_at
    }
}

/// Immutable description of one verified server executable.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReleaseManifest {
    schema: u32,
    server: String,
    generation: ReleaseId,
    binary_sha256: ReleaseId,
    executable: PathBuf,
    arguments: Vec<String>,
    state: StateCompatibility,
    provenance: ReleaseProvenance,
}

impl ReleaseManifest {
    /// Constructs a release manifest. The executable digest is verified separately.
    pub fn try_new(
        server: impl Into<String>,
        generation: ReleaseId,
        binary_sha256: ReleaseId,
        executable: PathBuf,
        arguments: Vec<String>,
        state: StateCompatibility,
        provenance: ReleaseProvenance,
    ) -> io::Result<Self> {
        let manifest = Self {
            schema: RELEASE_SCHEMA,
            server: server.into(),
            generation,
            binary_sha256,
            executable,
            arguments,
            state,
            provenance,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Returns the server name.
    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Returns the executable digest and generation identity.
    #[must_use]
    pub const fn generation(&self) -> &ReleaseId {
        &self.generation
    }

    /// Returns the exact executable digest.
    #[must_use]
    pub const fn binary_sha256(&self) -> &ReleaseId {
        &self.binary_sha256
    }

    /// Returns the immutable executable path.
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Returns the default server arguments used for a fresh launch.
    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    /// Returns the state compatibility contract.
    #[must_use]
    pub const fn state(&self) -> &StateCompatibility {
        &self.state
    }

    /// Returns exact source and toolchain provenance.
    #[must_use]
    pub const fn provenance(&self) -> &ReleaseProvenance {
        &self.provenance
    }

    fn validate(&self) -> io::Result<()> {
        if self.schema != RELEASE_SCHEMA {
            return Err(invalid_data(format!(
                "unsupported release manifest schema {}",
                self.schema
            )));
        }
        validate_server_name(&self.server)?;
        if !self.executable.is_absolute() {
            return Err(invalid_data("release executable path must be absolute"));
        }
        if self
            .arguments
            .iter()
            .any(|argument| argument.as_bytes().contains(&0))
        {
            return Err(invalid_data("release arguments cannot contain NUL bytes"));
        }
        Ok(())
    }
}

/// Mutable channel value selecting one immutable release manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReleasePointer {
    schema: u32,
    manifest: PathBuf,
}

impl ReleasePointer {
    /// Constructs a pointer to an absolute immutable manifest path.
    pub fn try_new(manifest: PathBuf) -> io::Result<Self> {
        if !manifest.is_absolute() {
            return Err(invalid_data("release manifest path must be absolute"));
        }
        Ok(Self {
            schema: RELEASE_SCHEMA,
            manifest,
        })
    }

    /// Returns the selected immutable manifest path.
    #[must_use]
    pub fn manifest(&self) -> &Path {
        &self.manifest
    }

    fn validate(&self) -> io::Result<()> {
        if self.schema != RELEASE_SCHEMA {
            return Err(invalid_data(format!(
                "unsupported release pointer schema {}",
                self.schema
            )));
        }
        if !self.manifest.is_absolute() {
            return Err(invalid_data("release manifest path must be absolute"));
        }
        Ok(())
    }
}

/// One observation of the executable source selected for a host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseObservation {
    /// The running executable remains selected.
    Incumbent,
    /// A directly replaced executable is still settling.
    SuccessorSettling,
    /// A complete, verified successor is ready for handoff.
    SuccessorReady,
}

impl ReleaseObservation {
    /// Returns whether any successor work is pending.
    #[must_use]
    pub const fn rollout_pending(self) -> bool {
        !matches!(self, Self::Incumbent)
    }

    /// Returns whether a successor can be handed the public session.
    #[must_use]
    pub const fn rollout_ready(self) -> bool {
        matches!(self, Self::SuccessorReady)
    }
}

/// Result of attempting a two-phase successor handoff.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandoffOutcome {
    /// No successor was armed; the incumbent retains the session.
    Retained,
    /// The successor reported live and the incumbent must stop reading immediately.
    Relinquish,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BinaryFingerprint {
    length_bytes: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

#[derive(Clone, Debug)]
struct SettlingSuccessor {
    fingerprint: BinaryFingerprint,
    first_observed_at: Instant,
}

#[derive(Clone, Debug)]
struct SuccessorTarget {
    #[cfg(unix)]
    executable: PathBuf,
    #[cfg(unix)]
    channel: Option<PathBuf>,
    generation: Option<ReleaseId>,
}

#[derive(Debug)]
enum ReleaseMode {
    Standalone {
        executable: PathBuf,
        incumbent: BinaryFingerprint,
        settling: Option<SettlingSuccessor>,
    },
    Managed {
        channel: PathBuf,
        server: String,
        incumbent: ReleaseId,
    },
}

/// Optional release runtime embedded by a standalone-capable MCP host.
///
/// With no managed environment, this observes only atomic replacement of the
/// current executable. A depot launcher activates channel polling by setting
/// [`LIBMCP_RELEASE_CHANNEL_ENV`] and [`LIBMCP_RELEASE_GENERATION_ENV`].
#[derive(Debug)]
pub struct ReleaseRuntime {
    mode: ReleaseMode,
    successor: Option<SuccessorTarget>,
    successor_gate: Option<PathBuf>,
}

impl ReleaseRuntime {
    /// Discovers standalone or managed execution for `server`.
    pub fn discover(server: &str) -> io::Result<Self> {
        validate_server_name(server)?;
        let channel = std::env::var_os(LIBMCP_RELEASE_CHANNEL_ENV);
        let generation = std::env::var_os(LIBMCP_RELEASE_GENERATION_ENV);
        let mode = match (channel, generation) {
            (None, None) => {
                let executable = std::env::current_exe()?;
                let incumbent = fingerprint(&executable)?;
                ReleaseMode::Standalone {
                    executable,
                    incumbent,
                    settling: None,
                }
            }
            (Some(channel), Some(generation)) => {
                let channel = PathBuf::from(channel);
                if !channel.is_absolute() {
                    return Err(invalid_data("managed release channel must be absolute"));
                }
                let generation = os_string(generation, "managed release generation")?;
                ReleaseMode::Managed {
                    channel,
                    server: server.to_owned(),
                    incumbent: ReleaseId::try_new(generation)?,
                }
            }
            (Some(_), None) | (None, Some(_)) => {
                return Err(invalid_data(
                    "managed release channel and generation must be set together",
                ));
            }
        };
        let successor_gate = std::env::var_os(LIBMCP_HANDOFF_SOCKET_ENV)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        Ok(Self {
            mode,
            successor: None,
            successor_gate,
        })
    }

    /// Returns whether the executable is selected through a managed channel.
    #[must_use]
    pub const fn is_managed(&self) -> bool {
        matches!(self.mode, ReleaseMode::Managed { .. })
    }

    /// Returns whether the launch path is durable rather than a Cargo target artifact.
    #[must_use]
    pub fn launch_path_stable(&self) -> bool {
        match &self.mode {
            ReleaseMode::Managed { .. } => true,
            ReleaseMode::Standalone { executable, .. } => !path_contains_target(executable),
        }
    }

    /// Returns the managed generation, if any.
    #[must_use]
    pub const fn generation(&self) -> Option<&ReleaseId> {
        match &self.mode {
            ReleaseMode::Managed { incumbent, .. } => Some(incumbent),
            ReleaseMode::Standalone { .. } => None,
        }
    }

    /// Observes the selected release without changing the running process.
    pub fn observe(&mut self) -> io::Result<ReleaseObservation> {
        match &mut self.mode {
            ReleaseMode::Managed {
                channel,
                server,
                incumbent,
            } => {
                let release = load_release(channel, server)?;
                if release.generation() == incumbent {
                    self.successor = None;
                    return Ok(ReleaseObservation::Incumbent);
                }
                if self
                    .successor
                    .as_ref()
                    .and_then(|target| target.generation.as_ref())
                    != Some(release.generation())
                {
                    verify_release(&release)?;
                    self.successor = Some(SuccessorTarget {
                        #[cfg(unix)]
                        executable: release.executable().to_owned(),
                        #[cfg(unix)]
                        channel: Some(channel.clone()),
                        generation: Some(release.generation().clone()),
                    });
                }
                Ok(ReleaseObservation::SuccessorReady)
            }
            ReleaseMode::Standalone {
                executable,
                incumbent,
                settling,
            } => {
                let observed = match fingerprint(executable) {
                    Ok(observed) => observed,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        *settling = None;
                        return Ok(ReleaseObservation::SuccessorSettling);
                    }
                    Err(error) => return Err(error),
                };
                if &observed == incumbent {
                    *settling = None;
                    self.successor = None;
                    return Ok(ReleaseObservation::Incumbent);
                }
                match settling {
                    Some(candidate)
                        if candidate.fingerprint == observed
                            && candidate.first_observed_at.elapsed() >= SUCCESSOR_SETTLE_TIME =>
                    {
                        self.successor = Some(SuccessorTarget {
                            #[cfg(unix)]
                            executable: executable.clone(),
                            #[cfg(unix)]
                            channel: None,
                            generation: None,
                        });
                        Ok(ReleaseObservation::SuccessorReady)
                    }
                    Some(candidate) if candidate.fingerprint == observed => {
                        Ok(ReleaseObservation::SuccessorSettling)
                    }
                    Some(_) | None => {
                        *settling = Some(SettlingSuccessor {
                            fingerprint: observed,
                            first_observed_at: Instant::now(),
                        });
                        Ok(ReleaseObservation::SuccessorSettling)
                    }
                }
            }
        }
    }

    /// Arms a same-release successor, primarily for deterministic recovery tests.
    pub fn arm_current_relaunch(&mut self) -> io::Result<()> {
        let generation = match &self.mode {
            ReleaseMode::Standalone { .. } => None,
            ReleaseMode::Managed { incumbent, .. } => Some(incumbent.clone()),
        };
        self.successor = Some(SuccessorTarget {
            #[cfg(unix)]
            executable: std::env::current_exe()?,
            #[cfg(unix)]
            channel: match &self.mode {
                ReleaseMode::Standalone { .. } => None,
                ReleaseMode::Managed { channel, .. } => Some(channel.clone()),
            },
            generation,
        });
        Ok(())
    }

    /// Releases a child host from its private readiness barrier.
    ///
    /// Call this after the host has loaded configuration and hydrated its
    /// snapshot but before it reads public stdin.
    pub fn admit_successor(&mut self) -> io::Result<()> {
        let Some(socket) = self.successor_gate.take() else {
            return Ok(());
        };
        #[cfg(unix)]
        {
            let mut stream = UnixStream::connect(socket)?;
            stream.set_read_timeout(Some(SUCCESSOR_GATE_TIMEOUT))?;
            stream.set_write_timeout(Some(SUCCESSOR_GATE_TIMEOUT))?;
            stream.write_all(&[READY])?;
            expect_gate_byte(&mut stream, ACTIVATE, "successor activation")?;
            stream.write_all(&[LIVE])?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = socket;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "live release handoff requires Unix-domain sockets",
            ))
        }
    }

    /// Starts an armed successor, waits for readiness, and transfers authority.
    ///
    /// `snapshot_env` names the consumer-owned variable from which the child
    /// hydrates `snapshot_path`. On [`HandoffOutcome::Relinquish`], the caller
    /// must return from its public read loop without reading another byte.
    pub fn handoff(
        &self,
        snapshot_env: &str,
        snapshot_path: &Path,
        timeout: Duration,
    ) -> io::Result<HandoffOutcome> {
        let Some(target) = self.successor.as_ref() else {
            return Ok(HandoffOutcome::Retained);
        };
        validate_env_name(snapshot_env)?;
        if timeout.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "successor handoff timeout must be non-zero",
            ));
        }
        #[cfg(unix)]
        {
            handoff_unix(target, snapshot_env, snapshot_path, timeout)
        }
        #[cfg(not(unix))]
        {
            let _ = (target, snapshot_env, snapshot_path, timeout);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "live release handoff requires Unix-domain sockets",
            ))
        }
    }
}

/// Loads and validates the immutable release selected by `channel_path`.
pub fn load_release(channel_path: &Path, expected_server: &str) -> io::Result<ReleaseManifest> {
    validate_server_name(expected_server)?;
    if !channel_path.is_absolute() {
        return Err(invalid_data("release channel path must be absolute"));
    }
    let pointer = serde_json::from_slice::<ReleasePointer>(&read_private_regular(
        channel_path,
        POINTER_MAX_BYTES,
    )?)
    .map_err(|error| invalid_data(format!("invalid release pointer: {error}")))?;
    pointer.validate()?;

    let root = channel_path
        .parent()
        .ok_or_else(|| invalid_data("release channel has no parent"))?;
    let releases = fs::canonicalize(root.join("releases"))?;
    let manifest_path = fs::canonicalize(pointer.manifest())?;
    if !manifest_path.starts_with(&releases) {
        return Err(permission_denied(
            "release manifest escapes the channel release root",
        ));
    }
    let manifest = serde_json::from_slice::<ReleaseManifest>(&read_private_regular(
        &manifest_path,
        MANIFEST_MAX_BYTES,
    )?)
    .map_err(|error| invalid_data(format!("invalid release manifest: {error}")))?;
    manifest.validate()?;
    if manifest.server() != expected_server {
        return Err(invalid_data(format!(
            "release server `{}` does not match expected `{expected_server}`",
            manifest.server()
        )));
    }
    let release_dir = manifest_path
        .parent()
        .ok_or_else(|| invalid_data("release manifest has no parent"))?;
    if release_dir.file_name().and_then(|name| name.to_str())
        != Some(manifest.generation().as_str())
    {
        return Err(invalid_data(
            "release directory does not match its generation",
        ));
    }
    let executable = fs::canonicalize(manifest.executable())?;
    if executable.parent() != Some(release_dir) {
        return Err(permission_denied(
            "release executable is not owned by its immutable release directory",
        ));
    }
    validate_executable(&executable)?;
    Ok(manifest)
}

/// Verifies that a release executable hashes to its generation identity.
pub fn verify_release(release: &ReleaseManifest) -> io::Result<()> {
    let mut executable = open_owned_regular(release.executable())?;
    validate_executable(release.executable())?;
    let mut hasher = Sha256::new();
    let _bytes = io::copy(&mut executable, &mut hasher)?;
    let observed = release_id_from_digest(hasher.finalize().as_slice())?;
    if &observed != release.binary_sha256() {
        return Err(invalid_data(format!(
            "release executable digest {observed} does not match {}",
            release.binary_sha256()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn handoff_unix(
    target: &SuccessorTarget,
    snapshot_env: &str,
    snapshot_path: &Path,
    timeout: Duration,
) -> io::Result<HandoffOutcome> {
    let rendezvous = tempfile::Builder::new()
        .prefix("libmcp-handoff-")
        .tempdir()?;
    let socket_path = rendezvous.path().join("gate");
    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;

    let mut command = Command::new(&target.executable);
    let _command = command
        .args(std::env::args_os().skip(1))
        .env(snapshot_env, snapshot_path)
        .env(LIBMCP_HANDOFF_SOCKET_ENV, &socket_path);
    match (&target.channel, &target.generation) {
        (Some(channel), Some(generation)) => {
            let _command = command
                .env(LIBMCP_RELEASE_CHANNEL_ENV, channel)
                .env(LIBMCP_RELEASE_GENERATION_ENV, generation.as_str());
        }
        (None, None) => {
            let _command = command
                .env_remove(LIBMCP_RELEASE_CHANNEL_ENV)
                .env_remove(LIBMCP_RELEASE_GENERATION_ENV);
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err(invalid_data("incomplete successor release target"));
        }
    }
    let mut child = command.spawn()?;
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| invalid_data("successor handoff deadline overflow"))?;
    let result = finish_handoff(&listener, &mut child, deadline);
    if result.is_err() {
        abort_successor(&mut child);
    }
    result.map(|()| HandoffOutcome::Relinquish)
}

#[cfg(unix)]
fn finish_handoff(listener: &UnixListener, child: &mut Child, deadline: Instant) -> io::Result<()> {
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _address)) => {
                stream.set_nonblocking(false)?;
                break stream;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait()? {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        format!("successor exited before readiness with {status}"),
                    ));
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "successor did not report readiness before the handoff deadline",
                    ));
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error),
        }
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "successor readiness consumed the handoff deadline",
        ));
    }
    stream.set_read_timeout(Some(remaining))?;
    stream.set_write_timeout(Some(remaining))?;
    expect_gate_byte(&mut stream, READY, "successor readiness")?;
    stream.write_all(&[ACTIVATE])?;
    expect_gate_byte(&mut stream, LIVE, "successor activation acknowledgement")
}

#[cfg(unix)]
fn expect_gate_byte(stream: &mut UnixStream, expected: u8, stage: &str) -> io::Result<()> {
    let mut observed = [0_u8; 1];
    stream.read_exact(&mut observed)?;
    if observed[0] != expected {
        return Err(invalid_data(format!(
            "invalid {stage} byte 0x{:02x}",
            observed[0]
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn abort_successor(child: &mut Child) {
    match child.try_wait() {
        Ok(Some(_status)) => {}
        Ok(None) | Err(_) => drop(child.kill()),
    }
    drop(child.wait());
}

fn validate_server_name(server: &str) -> io::Result<()> {
    if server.is_empty()
        || server.len() > 64
        || !server
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(invalid_data(
            "release server must be 1-64 portable filename characters",
        ));
    }
    Ok(())
}

fn validate_env_name(name: &str) -> io::Result<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "snapshot environment name must use uppercase ASCII, digits, and underscores",
        ));
    }
    Ok(())
}

fn os_string(value: OsString, field: &str) -> io::Result<String> {
    value
        .into_string()
        .map_err(|_| invalid_data(format!("{field} must be UTF-8")))
}

fn path_contains_target(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "target")
}

fn fingerprint(path: &Path) -> io::Result<BinaryFingerprint> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(invalid_data("release executable must be a regular file"));
    }
    Ok(BinaryFingerprint {
        length_bytes: metadata.len(),
        modified: metadata.modified()?,
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
    })
}

fn validate_executable(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(permission_denied(
            "release executable must be a regular file, not a symlink",
        ));
    }
    #[cfg(unix)]
    {
        if metadata.uid() != rustix::process::getuid().as_raw()
            || metadata.permissions().mode() & 0o022 != 0
            || metadata.permissions().mode() & 0o100 == 0
            || metadata.permissions().mode() & 0o6000 != 0
        {
            return Err(permission_denied(
                "release executable must be owner-executable, user-owned, and not writable by other principals",
            ));
        }
    }
    Ok(())
}

fn read_private_regular(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    let mut file = open_owned_regular(path)?;
    let metadata = file.metadata()?;
    if metadata.len() > max_bytes as u64 {
        return Err(invalid_data(format!(
            "release metadata exceeds {max_bytes} bytes"
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let _read = Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(invalid_data(format!(
            "release metadata exceeds {max_bytes} bytes"
        )));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn open_owned_regular(path: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, open};

    let descriptor = open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?;
    let file = File::from(descriptor);
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(permission_denied(
            "release file must be a user-owned regular file not writable by other principals",
        ));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_owned_regular(path: &Path) -> io::Result<File> {
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(permission_denied("release file must be a regular file"));
    }
    Ok(file)
}

fn release_id_from_digest(digest: &[u8]) -> io::Result<ReleaseId> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    if digest.len() != 32 {
        return Err(invalid_data("SHA-256 digest has an invalid width"));
    }
    let mut text = String::with_capacity(64);
    for byte in digest {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    ReleaseId::try_new(text)
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn permission_denied(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_epochs_govern_forward_and_reverse_compatibility() -> io::Result<()> {
        let epoch_1 = StateCompatibility::versioned([1], 1)?;
        let bridge = StateCompatibility::versioned([1, 2], 1)?;
        let epoch_2 = StateCompatibility::versioned([1, 2], 2)?;

        assert!(bridge.accepts(&epoch_1));
        assert!(epoch_2.accepts(&bridge));
        assert!(bridge.accepts(&epoch_2));
        assert!(!epoch_1.accepts(&epoch_2));
        assert!(!StateCompatibility::Stateless.accepts(&epoch_1));
        Ok(())
    }

    #[test]
    fn release_loader_confines_and_verifies_artifacts() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let server_root = root.path().join("mail");
        let source = std::env::current_exe()?;
        let binary_digest = digest_file(&source)?;
        let generation = ReleaseId::digest_bytes(b"mail release")?;
        let release_dir = server_root.join("releases").join(generation.as_str());
        fs::create_dir_all(&release_dir)?;
        let executable = release_dir.join("server");
        let _copied = fs::copy(source, &executable)?;
        set_private_permissions(&executable, 0o700)?;
        let manifest = ReleaseManifest::try_new(
            "mail",
            generation,
            binary_digest,
            executable,
            vec!["mcp".to_owned(), "serve".to_owned()],
            StateCompatibility::Stateless,
            provenance(root.path())?,
        )?;
        let manifest_path = release_dir.join("manifest.json");
        fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
        set_private_permissions(&manifest_path, 0o600)?;
        let pointer = ReleasePointer::try_new(manifest_path)?;
        let channel = server_root.join("current.json");
        fs::write(&channel, serde_json::to_vec(&pointer)?)?;
        set_private_permissions(&channel, 0o600)?;

        let loaded = load_release(&channel, "mail")?;
        verify_release(&loaded)?;
        assert_eq!(loaded, manifest);
        assert!(load_release(&channel, "papercuts").is_err());
        Ok(())
    }

    fn digest_file(path: &Path) -> io::Result<ReleaseId> {
        ReleaseId::digest_file(path)
    }

    fn provenance(root: &Path) -> io::Result<ReleaseProvenance> {
        ReleaseProvenance::try_new(
            root.to_owned(),
            "1111111111111111111111111111111111111111",
            ReleaseId::try_new("2".repeat(64))?,
            "rustc test",
            "2026-08-09T00:00:00Z",
        )
    }

    #[cfg(unix)]
    fn set_private_permissions(path: &Path, mode: u32) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }

    #[cfg(not(unix))]
    fn set_private_permissions(_path: &Path, _mode: u32) -> io::Result<()> {
        Ok(())
    }
}
