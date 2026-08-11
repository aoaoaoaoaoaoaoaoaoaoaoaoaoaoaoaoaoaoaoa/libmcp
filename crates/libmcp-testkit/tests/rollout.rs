//! Failure-atomic release-channel and live-handoff acceptance tests.
#![cfg(unix)]

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use libmcp::{
    LIBMCP_RELEASE_CHANNEL_ENV, LIBMCP_RELEASE_GENERATION_ENV, ReleaseId, ReleaseManifest,
    ReleasePointer, ReleaseProvenance, StateCompatibility,
};
use libmcp_testkit as _;
use serde as _;

#[test]
fn successor_hydrates_before_the_incumbent_relinquishes() -> io::Result<()> {
    let fixture = fixture()?;
    let parent = fixture.root.path().join("parent");
    let child = fixture.root.path().join("child");
    let status = Command::new(binary())
        .arg("parent")
        .env(LIBMCP_RELEASE_CHANNEL_ENV, &fixture.channel)
        .env(LIBMCP_RELEASE_GENERATION_ENV, "0".repeat(64))
        .env("LIBMCP_TEST_PARENT_MARKER", &parent)
        .env("LIBMCP_TEST_CHILD_MARKER", &child)
        .status()?;
    assert!(status.success());
    assert_eq!(fs::read_to_string(parent)?, "relinquish\n");
    wait_for(&child)?;
    assert_eq!(fs::read_to_string(child)?, "live\n");
    Ok(())
}

#[test]
fn failed_successor_leaves_the_incumbent_authoritative() -> io::Result<()> {
    let fixture = fixture()?;
    let parent = fixture.root.path().join("parent-failed");
    let child = fixture.root.path().join("child-failed");
    let status = Command::new(binary())
        .arg("parent")
        .env(LIBMCP_RELEASE_CHANNEL_ENV, &fixture.channel)
        .env(LIBMCP_RELEASE_GENERATION_ENV, "0".repeat(64))
        .env("LIBMCP_TEST_PARENT_MARKER", &parent)
        .env("LIBMCP_TEST_CHILD_MARKER", &child)
        .env("LIBMCP_TEST_HANDOFF_FAIL", "1")
        .status()?;
    assert!(status.success());
    assert!(fs::read_to_string(parent)?.starts_with("retained:"));
    assert!(!child.exists());
    Ok(())
}

struct Fixture {
    root: tempfile::TempDir,
    channel: PathBuf,
}

fn fixture() -> io::Result<Fixture> {
    let root = tempfile::tempdir()?;
    let source = binary();
    let binary_sha256 = ReleaseId::digest_file(&source)?;
    let generation = ReleaseId::digest_bytes(b"handoff fixture release")?;
    let server_root = root.path().join("handoff-fixture");
    let release_dir = server_root.join("releases").join(generation.as_str());
    fs::create_dir_all(&release_dir)?;
    let executable = release_dir.join("server");
    let _copied = fs::copy(&source, &executable)?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
    let provenance = ReleaseProvenance::try_new(
        root.path().to_owned(),
        "1111111111111111111111111111111111111111",
        ReleaseId::try_new("2".repeat(64))?,
        "rustc test",
        "2026-08-09T00:00:00Z",
    )?;
    let manifest = ReleaseManifest::try_new(
        "handoff-fixture",
        generation,
        binary_sha256,
        executable,
        vec!["parent".to_owned()],
        StateCompatibility::Stateless,
        provenance,
    )?;
    let manifest_path = release_dir.join("manifest.json");
    fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
    fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o600))?;
    let channel = server_root.join("current.json");
    let pointer = ReleasePointer::try_new(manifest_path)?;
    fs::write(&channel, serde_json::to_vec(&pointer)?)?;
    fs::set_permissions(&channel, fs::Permissions::from_mode(0o600))?;
    Ok(Fixture { root, channel })
}

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_handoff-fixture"))
}

fn wait_for(path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{} was not created", path.display()),
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
