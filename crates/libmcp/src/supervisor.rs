//! Stable public MCP host with disposable full-MCP business workers.

use crate::{
    DispatchQueueOutcome, EffectRecovery, FrameLimit, FrameReadOutcome, FramedMessage,
    HostRejection, HostSessionKernel, LIBMCP_HANDOFF_SOCKET_ENV, LIBMCP_RELEASE_CHANNEL_ENV,
    LIBMCP_RELEASE_GENERATION_ENV, ReleaseId, ReleaseManifest, ReplayBudget, RequestId,
    RpcEnvelopeKind, SessionPhase, SessionStateContract, ToolCatalog, load_release,
    parse_tool_call_meta, verify_release, write_frame,
};
use serde_json::{Map, Value, json};
use std::{collections::HashMap, ffi::OsString, io, path::PathBuf, process::Stdio, time::Duration};
use thiserror::Error;
use tokio::{
    io::{AsyncWrite, BufReader},
    process::{Child, ChildStdin, Command},
    sync::mpsc,
    time::{Instant, interval},
};

const EVENT_CAPACITY: usize = 64;
const WORKER_COMMAND_CAPACITY: usize = 8;
const QUEUE_CAPACITY: usize = 128;
const PENDING_CAPACITY: usize = 128;
const JOURNAL_MAX_ENTRIES: usize = 32;
const JOURNAL_MAX_BYTES: usize = 1024 * 1024;
const MAX_REPLAY_ATTEMPTS: u8 = 1;
const CHANNEL_POLL: Duration = Duration::from_millis(500);
const CANDIDATE_TIMEOUT: Duration = Duration::from_secs(15);
const CANDIDATE_RETENTION: Duration = Duration::from_mins(2);
const RECOVERY_BACKOFF: Duration = Duration::from_secs(1);

/// Complete launch contract for one stable supervised MCP session.
#[derive(Clone, Debug)]
pub struct SupervisorConfig {
    server: String,
    channel: PathBuf,
    initial: ReleaseManifest,
    argument_overrides: Vec<OsString>,
    frame_limit: FrameLimit,
}

impl SupervisorConfig {
    /// Constructs a supervisor over one verified initial release and mutable channel.
    pub fn try_new(
        server: impl Into<String>,
        channel: PathBuf,
        initial: ReleaseManifest,
        argument_overrides: Vec<OsString>,
    ) -> Result<Self, SupervisorError> {
        let server = server.into();
        if server.is_empty() || initial.server() != server {
            return Err(SupervisorError::Contract(
                "supervisor server and initial release disagree".to_owned(),
            ));
        }
        if !channel.is_absolute() {
            return Err(SupervisorError::Contract(
                "supervisor release channel must be absolute".to_owned(),
            ));
        }
        verify_release(&initial)?;
        Ok(Self {
            server,
            channel,
            initial,
            argument_overrides,
            frame_limit: FrameLimit::DEFAULT,
        })
    }

    /// Replaces the default eight-mebibyte frame bound.
    #[must_use]
    pub const fn with_frame_limit(mut self, frame_limit: FrameLimit) -> Self {
        self.frame_limit = frame_limit;
        self
    }
}

/// Terminal failure of the stable host itself.
#[derive(Debug, Error)]
pub enum SupervisorError {
    /// Operating-system or transport failure in the host nucleus.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// Static or release contract was inconsistent.
    #[error("supervisor contract violation: {0}")]
    Contract(String),
    /// Public JSON-RPC state contradicted the continuity kernel.
    #[error("public session invariant failed: {0}")]
    Session(#[from] HostRejection),
    /// JSON serialization failed inside a host-owned protocol message.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Runs the supervisor to public end-of-stream.
pub fn run_supervised(config: SupervisorConfig) -> Result<(), SupervisorError> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(Supervisor::start(config))
}

#[derive(Debug)]
enum Event {
    ClientFrame(Vec<u8>),
    ClientEnd,
    ClientFault(io::Error),
    WorkerFrame { token: u64, payload: Vec<u8> },
    WorkerGone { token: u64, detail: String },
}

#[derive(Debug)]
enum WorkerCommand {
    Frame(Vec<u8>),
    Kill,
}

#[derive(Debug)]
struct WorkerHandle {
    token: u64,
    release: ReleaseManifest,
    sender: mpsc::Sender<WorkerCommand>,
    catalog: Option<ToolCatalog>,
}

impl WorkerHandle {
    async fn send(&self, frame: Vec<u8>) -> Result<(), SupervisorError> {
        self.sender
            .send(WorkerCommand::Frame(frame))
            .await
            .map_err(|_| SupervisorError::Contract("worker command channel closed".to_owned()))
    }

    fn kill(&self) {
        let _sent = self.sender.try_send(WorkerCommand::Kill);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CandidatePurpose {
    Rollover,
    Recovery,
}

#[derive(Debug)]
enum CandidateStage {
    Initializing,
    Catalog,
    Ready,
    Restoring,
}

#[derive(Debug)]
struct Candidate {
    worker: WorkerHandle,
    purpose: CandidatePurpose,
    stage: CandidateStage,
    deadline: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RolloverPhase {
    None,
    AwaitingCatalogRefresh,
    Draining,
    Recovering,
}

#[derive(Debug)]
enum InternalAction {
    ActiveCatalog,
    CandidateInitialize,
    CandidateCatalog,
    RestoreJournal { index: usize },
}

#[derive(Debug)]
struct InternalRequest {
    worker: u64,
    action: InternalAction,
}

#[derive(Debug)]
struct CallbackRoute {
    worker: u64,
    original: RequestId,
}

#[derive(Clone, Debug)]
struct JournalEntry {
    key: String,
    frame: FramedMessage,
}

#[derive(Debug, Default)]
struct SessionJournal {
    entries: Vec<JournalEntry>,
    bytes: usize,
    pinned: bool,
    checkpointed: bool,
}

impl SessionJournal {
    fn record(&mut self, key: &str, frame: FramedMessage) -> Result<(), SupervisorError> {
        if let Some(index) = self.entries.iter().position(|entry| entry.key == key) {
            let removed = self.entries.remove(index);
            self.bytes = self.bytes.saturating_sub(removed.frame.payload().len());
        }
        let next_bytes = self
            .bytes
            .checked_add(frame.payload().len())
            .ok_or_else(|| SupervisorError::Contract("session journal size overflow".to_owned()))?;
        if self.entries.len() >= JOURNAL_MAX_ENTRIES || next_bytes > JOURNAL_MAX_BYTES {
            return Err(SupervisorError::Contract(
                "session journal capacity exhausted".to_owned(),
            ));
        }
        self.bytes = next_bytes;
        self.entries.push(JournalEntry {
            key: key.to_owned(),
            frame,
        });
        Ok(())
    }
}

#[derive(Debug)]
struct Supervisor {
    config: SupervisorConfig,
    kernel: HostSessionKernel,
    active: Option<WorkerHandle>,
    candidate: Option<Candidate>,
    published_catalog: Option<ToolCatalog>,
    internal: HashMap<RequestId, InternalRequest>,
    callbacks: HashMap<RequestId, CallbackRoute>,
    journal: SessionJournal,
    phase: RolloverPhase,
    next_worker: u64,
    next_internal: u64,
    next_callback: u64,
    retry_at: Option<Instant>,
    recovery_release: ReleaseManifest,
    rejected_generation: Option<ReleaseId>,
    refresh_request: Option<FramedMessage>,
    events: mpsc::Sender<Event>,
}

impl Supervisor {
    async fn start(config: SupervisorConfig) -> Result<(), SupervisorError> {
        let (events, mut receiver) = mpsc::channel(EVENT_CAPACITY);
        spawn_client_reader(events.clone(), config.frame_limit);
        let mut supervisor = Self {
            config: config.clone(),
            kernel: HostSessionKernel::cold(),
            active: None,
            candidate: None,
            published_catalog: None,
            internal: HashMap::new(),
            callbacks: HashMap::new(),
            journal: SessionJournal::default(),
            phase: RolloverPhase::None,
            next_worker: 0,
            next_internal: 0,
            next_callback: 0,
            retry_at: None,
            recovery_release: config.initial.clone(),
            rejected_generation: None,
            refresh_request: None,
            events,
        };
        let initial = supervisor.spawn_worker(config.initial).await?;
        supervisor.active = Some(initial);

        let mut stdout = tokio::io::stdout();
        let mut ticker = interval(CHANNEL_POLL);
        loop {
            tokio::select! {
                event = receiver.recv() => {
                    let Some(event) = event else {
                        return Err(SupervisorError::Contract("supervisor event channel closed".to_owned()));
                    };
                    match event {
                        Event::ClientFrame(payload) => {
                            supervisor.handle_client_payload(payload, &mut stdout).await?;
                        }
                        Event::ClientEnd => {
                            supervisor.shutdown();
                            return Ok(());
                        }
                        Event::ClientFault(error) => {
                            supervisor.shutdown();
                            return Err(error.into());
                        }
                        Event::WorkerFrame { token, payload } => {
                            supervisor.handle_worker_payload(token, payload, &mut stdout).await?;
                        }
                        Event::WorkerGone { token, detail } => {
                            supervisor.handle_worker_loss(token, &detail, &mut stdout).await?;
                        }
                    }
                }
                _ = ticker.tick() => {
                    supervisor.on_tick(&mut stdout).await?;
                }
            }
        }
    }

    async fn spawn_worker(
        &mut self,
        release: ReleaseManifest,
    ) -> Result<WorkerHandle, SupervisorError> {
        verify_release(&release)?;
        self.next_worker = self
            .next_worker
            .checked_add(1)
            .ok_or_else(|| SupervisorError::Contract("worker identity exhausted".to_owned()))?;
        let token = self.next_worker;
        let (sender, receiver) = mpsc::channel(WORKER_COMMAND_CAPACITY);
        let mut command = Command::new(release.executable());
        let child = command
            .args(release.arguments())
            .args(&self.config.argument_overrides)
            .env_remove(LIBMCP_RELEASE_CHANNEL_ENV)
            .env_remove(LIBMCP_RELEASE_GENERATION_ENV)
            .env_remove(LIBMCP_HANDOFF_SOCKET_ENV)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        spawn_worker_io(
            token,
            child,
            receiver,
            self.events.clone(),
            self.config.frame_limit,
        )?;
        Ok(WorkerHandle {
            token,
            release,
            sender,
            catalog: None,
        })
    }

    async fn handle_client_payload<W>(
        &mut self,
        payload: Vec<u8>,
        stdout: &mut W,
    ) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        let frame = match FramedMessage::parse(payload) {
            Ok(frame) => frame,
            Err(error) => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": error.to_string()}
                });
                return write_value(stdout, &response, self.config.frame_limit).await;
            }
        };
        if let Err(rejection) = self.kernel.observe_client_frame(&frame) {
            return self.reject_client_frame(&frame, rejection, stdout).await;
        }

        match frame.classify() {
            RpcEnvelopeKind::Response { id, .. } => {
                self.route_callback_response(&id, frame, stdout).await
            }
            RpcEnvelopeKind::Notification { ref method }
                if method.is_initialized_notification() =>
            {
                let token = self.active_token();
                if let Err(error) = self.send_active(frame.payload().to_vec()).await {
                    return self
                        .handle_worker_loss_by_id(token, &error.to_string(), stdout)
                        .await;
                }
                if let Err(error) = self.request_active_catalog().await {
                    self.handle_worker_loss_by_id(token, &error.to_string(), stdout)
                        .await?;
                }
                Ok(())
            }
            RpcEnvelopeKind::Notification { ref method }
                if method.as_str() == "notifications/cancelled" =>
            {
                self.route_cancellation(frame, stdout).await
            }
            RpcEnvelopeKind::Request { ref method, .. }
                if method.as_str() == "tools/list" && self.published_catalog.is_some() =>
            {
                if self.phase == RolloverPhase::AwaitingCatalogRefresh {
                    self.refresh_request = Some(frame);
                    self.phase = RolloverPhase::Draining;
                    self.try_begin_restore(stdout).await
                } else if self.gating_new_work() {
                    self.queue_or_reject(frame, stdout).await
                } else {
                    self.reply_catalog(&frame, stdout).await
                }
            }
            RpcEnvelopeKind::Request { ref method, .. }
                if !method.is_initialize() && self.published_catalog.is_none() =>
            {
                self.queue_or_reject(frame, stdout).await
            }
            RpcEnvelopeKind::Request { .. } | RpcEnvelopeKind::Notification { .. }
                if self.gating_new_work() || self.active.is_none() =>
            {
                self.queue_or_reject(frame, stdout).await
            }
            RpcEnvelopeKind::Request { id, ref method } => {
                let replay = self.classify_request(&frame, method);
                if let Err(rejection) =
                    self.kernel
                        .begin_request_dispatch(&frame, replay, PENDING_CAPACITY)
                {
                    return self.reject_client_frame(&frame, rejection, stdout).await;
                }
                if let Err(error) = self.send_active(frame.payload().to_vec()).await {
                    self.handle_worker_loss_by_id(self.active_token(), &error.to_string(), stdout)
                        .await?;
                }
                let _ = id;
                Ok(())
            }
            RpcEnvelopeKind::Notification { .. } => {
                let token = self.active_token();
                if let Err(error) = self.send_active(frame.payload().to_vec()).await {
                    self.handle_worker_loss_by_id(token, &error.to_string(), stdout)
                        .await?;
                }
                Ok(())
            }
        }
    }

    async fn handle_worker_payload<W>(
        &mut self,
        token: u64,
        payload: Vec<u8>,
        stdout: &mut W,
    ) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        let frame = match FramedMessage::parse(payload) {
            Ok(frame) => frame,
            Err(error) => {
                let detail = format!("worker {token} emitted invalid JSON-RPC: {error}");
                if self
                    .candidate
                    .as_ref()
                    .is_some_and(|candidate| candidate.worker.token == token)
                {
                    return self.reject_candidate(&detail, stdout).await;
                }
                return self
                    .handle_worker_loss_by_id(Some(token), &detail, stdout)
                    .await;
            }
        };
        match frame.classify() {
            RpcEnvelopeKind::Response { ref id, has_error } => {
                if let Some(internal) = self.internal.remove(id) {
                    if internal.worker != token {
                        return Err(SupervisorError::Contract(
                            "internal response crossed worker generations".to_owned(),
                        ));
                    }
                    return self
                        .handle_internal_response(internal, frame, has_error, stdout)
                        .await;
                }
                if self.active_token() != Some(token) {
                    return Ok(());
                }
                let completed = match self.kernel.complete_response(&frame) {
                    Ok(completed) => completed,
                    Err(error) => {
                        return self
                            .handle_worker_loss_by_id(Some(token), &error.to_string(), stdout)
                            .await;
                    }
                };
                if !has_error
                    && let Err(error) =
                        self.record_session_transition(completed.request().frame().clone())
                {
                    self.journal.pinned = true;
                    eprintln!(
                        "libmcp: {}: session journal sealed after failure: {error}",
                        self.config.server
                    );
                }
                let outgoing = if completed.request().method().is_initialize() {
                    patch_initialize_response(frame.value().clone())
                } else {
                    frame.value().clone()
                };
                write_value(stdout, &outgoing, self.config.frame_limit).await?;
                self.try_begin_restore(stdout).await
            }
            RpcEnvelopeKind::Request { id, .. } => {
                if self.active_token() != Some(token) {
                    if self
                        .candidate
                        .as_ref()
                        .is_some_and(|candidate| candidate.worker.token == token)
                    {
                        return self
                            .reject_candidate("candidate issued a client request", stdout)
                            .await;
                    }
                    let _ = id;
                    return Ok(());
                }
                let public_id = self.next_callback_id()?;
                let rewritten = rewrite_id(frame.value(), &public_id)?;
                let _previous = self.callbacks.insert(
                    public_id,
                    CallbackRoute {
                        worker: token,
                        original: id,
                    },
                );
                write_value(stdout, &rewritten, self.config.frame_limit).await
            }
            RpcEnvelopeKind::Notification { .. } => {
                if self.active_token() == Some(token) {
                    write_frame(stdout, frame.payload(), self.config.frame_limit).await?;
                }
                Ok(())
            }
        }
    }

    async fn handle_internal_response<W>(
        &mut self,
        internal: InternalRequest,
        frame: FramedMessage,
        has_error: bool,
        stdout: &mut W,
    ) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        match internal.action {
            InternalAction::ActiveCatalog => {
                if has_error {
                    return self
                        .handle_worker_loss_by_id(
                            Some(internal.worker),
                            "active worker rejected tools/list",
                            stdout,
                        )
                        .await;
                }
                let catalog = match parse_catalog_response(&frame) {
                    Ok(catalog) => catalog,
                    Err(error) => {
                        return self
                            .handle_worker_loss_by_id(
                                Some(internal.worker),
                                &error.to_string(),
                                stdout,
                            )
                            .await;
                    }
                };
                if let Some(active) = self.active.as_mut() {
                    active.catalog = Some(catalog.clone());
                }
                self.published_catalog = Some(catalog);
                self.flush_queue(stdout).await
            }
            InternalAction::CandidateInitialize => {
                eprintln!("libmcp: {}: candidate initialized", self.config.server);
                if has_error {
                    return self
                        .reject_candidate("candidate rejected initialize", stdout)
                        .await;
                }
                if self.kernel.session_phase() == SessionPhase::AwaitingInitialized {
                    return self.activate_candidate(stdout).await;
                }
                if let Err(error) = self.seed_candidate_catalog().await {
                    return self.reject_candidate(&error.to_string(), stdout).await;
                }
                Ok(())
            }
            InternalAction::CandidateCatalog => {
                eprintln!("libmcp: {}: candidate catalog received", self.config.server);
                if has_error {
                    return self
                        .reject_candidate("candidate rejected tools/list", stdout)
                        .await;
                }
                let catalog = match parse_catalog_response(&frame) {
                    Ok(catalog) => catalog,
                    Err(error) => return self.reject_candidate(&error.to_string(), stdout).await,
                };
                let Some(candidate) = self.candidate.as_mut() else {
                    return Ok(());
                };
                candidate.worker.catalog = Some(catalog.clone());
                candidate.stage = CandidateStage::Ready;
                candidate.deadline = Instant::now() + CANDIDATE_RETENTION;
                self.on_candidate_ready(catalog, stdout).await
            }
            InternalAction::RestoreJournal { index } => {
                if has_error {
                    return self
                        .reject_candidate("candidate rejected session journal", stdout)
                        .await;
                }
                if let Err(error) = self.continue_restore(index + 1, stdout).await {
                    return self.reject_candidate(&error.to_string(), stdout).await;
                }
                Ok(())
            }
        }
    }

    async fn on_candidate_ready<W>(
        &mut self,
        catalog: ToolCatalog,
        stdout: &mut W,
    ) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        let same = match self.published_catalog.as_ref() {
            Some(active) => active.canonical_bytes()? == catalog.canonical_bytes()?,
            None => false,
        };
        if same {
            eprintln!(
                "libmcp: {}: candidate {} ready; draining incumbent",
                self.config.server,
                self.candidate
                    .as_ref()
                    .map(|candidate| candidate.worker.release.generation().as_str())
                    .unwrap_or("unknown")
            );
            self.phase = RolloverPhase::Draining;
            return self.try_begin_restore(stdout).await;
        }

        let notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/tools/list_changed"
        });
        eprintln!(
            "libmcp: {}: candidate catalog changed; awaiting client refresh",
            self.config.server
        );
        write_value(stdout, &notification, self.config.frame_limit).await?;
        self.phase = RolloverPhase::AwaitingCatalogRefresh;
        Ok(())
    }

    async fn try_begin_restore<W>(&mut self, stdout: &mut W) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        if self.phase != RolloverPhase::Draining
            || self.kernel.has_in_flight_dispatches()
            || self.active_callbacks() != 0
        {
            return Ok(());
        }
        if self.journal.pinned || self.journal.checkpointed {
            return self
                .reject_candidate(
                    "incumbent session state is not generically migratable",
                    stdout,
                )
                .await;
        }
        self.continue_restore(0, stdout).await
    }

    async fn continue_restore<W>(
        &mut self,
        index: usize,
        stdout: &mut W,
    ) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        if index >= self.journal.entries.len() {
            return self.activate_candidate(stdout).await;
        }
        let Some(candidate) = self.candidate.as_mut() else {
            return Ok(());
        };
        candidate.stage = CandidateStage::Restoring;
        let token = candidate.worker.token;
        let id = self.internal_id(token, "journal")?;
        let frame = rewrite_id(self.journal.entries[index].frame.value(), &id)?;
        let _previous = self.internal.insert(
            id,
            InternalRequest {
                worker: token,
                action: InternalAction::RestoreJournal { index },
            },
        );
        self.send_worker(token, serialize(&frame)?).await
    }

    async fn activate_candidate<W>(&mut self, stdout: &mut W) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        let Some(candidate) = self.candidate.take() else {
            return Ok(());
        };
        self.published_catalog.clone_from(&candidate.worker.catalog);
        self.recovery_release = candidate.worker.release.clone();
        let incumbent = self.active.replace(candidate.worker);
        if let Some(incumbent) = incumbent {
            self.callbacks
                .retain(|_, route| route.worker != incumbent.token);
            incumbent.kill();
        }
        if let Some(active) = self.active.as_ref() {
            eprintln!(
                "libmcp: {}: activated generation {}",
                self.config.server,
                active.release.generation()
            );
        }
        self.phase = RolloverPhase::None;
        self.retry_at = None;
        if let Some(refresh) = self.refresh_request.take() {
            self.reply_catalog(&refresh, stdout).await?;
        }
        self.flush_queue(stdout).await
    }

    async fn handle_worker_loss<W>(
        &mut self,
        token: u64,
        detail: &str,
        stdout: &mut W,
    ) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        if self
            .candidate
            .as_ref()
            .is_some_and(|value| value.worker.token == token)
        {
            let purpose = self
                .candidate
                .as_ref()
                .map(|value| value.purpose)
                .unwrap_or(CandidatePurpose::Rollover);
            if purpose == CandidatePurpose::Rollover {
                return self.reject_candidate(detail, stdout).await;
            }
            self.candidate = None;
            self.internal.retain(|_, value| value.worker != token);
            self.phase = RolloverPhase::Recovering;
            self.retry_at = Some(Instant::now() + RECOVERY_BACKOFF);
            eprintln!("libmcp: {}: candidate lost: {detail}", self.config.server);
            return Ok(());
        }
        self.handle_worker_loss_by_id(Some(token), detail, stdout)
            .await
    }

    async fn handle_worker_loss_by_id<W>(
        &mut self,
        token: Option<u64>,
        detail: &str,
        stdout: &mut W,
    ) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        if token.is_none() || self.active_token() != token {
            return Ok(());
        }
        let lost = self.active.take();
        if let Some(lost) = lost {
            self.callbacks.retain(|_, route| route.worker != lost.token);
        }
        if let Some(candidate) = self.candidate.take() {
            candidate.worker.kill();
        }
        self.internal.clear();
        self.phase = RolloverPhase::Recovering;
        let outcome = self.kernel.requeue_pending_for_replay(ReplayBudget {
            max_attempts: MAX_REPLAY_ATTEMPTS,
            queue_capacity: QUEUE_CAPACITY,
        });
        for rejected in outcome.rejected {
            let response = rpc_error(
                &rejected.request_id,
                rejected.reason.code(),
                rejected.reason.message(),
            );
            write_value(stdout, &response, self.config.frame_limit).await?;
        }
        eprintln!("libmcp: {}: worker lost: {detail}", self.config.server);
        self.retry_at = Some(Instant::now());
        Ok(())
    }

    async fn start_recovery<W>(&mut self, stdout: &mut W) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        let release = self.recovery_release.clone();
        verify_release(&release)?;
        let worker = self.spawn_worker(release).await?;
        if matches!(
            self.kernel.session_phase(),
            SessionPhase::Cold | SessionPhase::Initializing
        ) {
            self.active = Some(worker);
            self.phase = RolloverPhase::None;
            self.retry_at = None;
            return self.flush_queue(stdout).await;
        }
        let token = worker.token;
        self.candidate = Some(Candidate {
            worker,
            purpose: CandidatePurpose::Recovery,
            stage: CandidateStage::Initializing,
            deadline: Instant::now() + CANDIDATE_TIMEOUT,
        });
        if let Err(error) = self.seed_candidate_initialize(token).await {
            if let Some(candidate) = self.candidate.take() {
                candidate.worker.kill();
            }
            self.internal.retain(|_, value| value.worker != token);
            return Err(error);
        }
        Ok(())
    }

    async fn on_tick<W>(&mut self, stdout: &mut W) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        if let Some(candidate) = self.candidate.as_ref()
            && Instant::now() >= candidate.deadline
        {
            let purpose = candidate.purpose;
            self.reject_candidate("candidate deadline expired", stdout)
                .await?;
            if purpose == CandidatePurpose::Recovery {
                self.phase = RolloverPhase::Recovering;
                self.retry_at = Some(Instant::now() + RECOVERY_BACKOFF);
            }
        }
        if self.phase == RolloverPhase::Recovering
            && self.candidate.is_none()
            && self
                .retry_at
                .is_none_or(|deadline| Instant::now() >= deadline)
        {
            self.retry_at = None;
            if let Err(error) = self.start_recovery(stdout).await {
                eprintln!(
                    "libmcp: {}: recovery start failed: {error}",
                    self.config.server
                );
                self.retry_at = Some(Instant::now() + RECOVERY_BACKOFF);
            }
            return Ok(());
        }
        if self.phase != RolloverPhase::None
            || self.candidate.is_some()
            || self.kernel.session_phase() != SessionPhase::Live
            || self.published_catalog.is_none()
        {
            return Ok(());
        }
        let release = match load_release(&self.config.channel, &self.config.server) {
            Ok(release) => release,
            Err(error) => {
                eprintln!(
                    "libmcp: {}: channel observation rejected: {error}",
                    self.config.server
                );
                return Ok(());
            }
        };
        let Some(active) = self.active.as_ref() else {
            return Ok(());
        };
        if release.generation() == active.release.generation() {
            return Ok(());
        }
        if self.rejected_generation.as_ref() == Some(release.generation()) {
            return Ok(());
        }
        if !release.state().accepts(active.release.state()) {
            eprintln!(
                "libmcp: {}: generation {} cannot read incumbent state",
                self.config.server,
                release.generation()
            );
            self.rejected_generation = Some(release.generation().clone());
            return Ok(());
        }
        if let Err(error) = verify_release(&release) {
            eprintln!(
                "libmcp: {}: candidate verification failed: {error}",
                self.config.server
            );
            self.rejected_generation = Some(release.generation().clone());
            return Ok(());
        }
        let generation = release.generation().clone();
        let worker = match self.spawn_worker(release).await {
            Ok(worker) => worker,
            Err(error) => {
                self.rejected_generation = Some(generation);
                eprintln!(
                    "libmcp: {}: candidate launch failed: {error}",
                    self.config.server
                );
                return Ok(());
            }
        };
        let token = worker.token;
        eprintln!(
            "libmcp: {}: preparing generation {}",
            self.config.server,
            worker.release.generation()
        );
        self.candidate = Some(Candidate {
            worker,
            purpose: CandidatePurpose::Rollover,
            stage: CandidateStage::Initializing,
            deadline: Instant::now() + CANDIDATE_TIMEOUT,
        });
        if let Err(error) = self.seed_candidate_initialize(token).await {
            self.reject_candidate(&error.to_string(), stdout).await?;
        }
        Ok(())
    }

    async fn seed_candidate_initialize(&mut self, token: u64) -> Result<(), SupervisorError> {
        let seed = self.kernel.replay_seed()?.ok_or_else(|| {
            SupervisorError::Contract("candidate has no initialization seed".to_owned())
        })?;
        let id = self.internal_id(token, "initialize")?;
        let initialize = FramedMessage::parse(seed.initialize_request().payload().to_vec())
            .map_err(|error| SupervisorError::Contract(error.to_string()))?;
        let rewritten = rewrite_id(initialize.value(), &id)?;
        let _previous = self.internal.insert(
            id,
            InternalRequest {
                worker: token,
                action: InternalAction::CandidateInitialize,
            },
        );
        self.send_worker(token, serialize(&rewritten)?).await
    }

    async fn seed_candidate_catalog(&mut self) -> Result<(), SupervisorError> {
        let Some(candidate) = self.candidate.as_mut() else {
            return Ok(());
        };
        let seed = self.kernel.replay_seed()?.ok_or_else(|| {
            SupervisorError::Contract("candidate has no initialization seed".to_owned())
        })?;
        let initialized = seed.initialized_notification().ok_or_else(|| {
            SupervisorError::Contract("candidate has no initialized notification".to_owned())
        })?;
        candidate.worker.send(initialized.to_vec()).await?;
        candidate.stage = CandidateStage::Catalog;
        let token = candidate.worker.token;
        let id = self.internal_id(token, "tools")?;
        let request = json!({"jsonrpc": "2.0", "id": id.to_json_value(), "method": "tools/list"});
        let _previous = self.internal.insert(
            id,
            InternalRequest {
                worker: token,
                action: InternalAction::CandidateCatalog,
            },
        );
        self.send_worker(token, serialize(&request)?).await
    }

    async fn request_active_catalog(&mut self) -> Result<(), SupervisorError> {
        let token = self
            .active_token()
            .ok_or_else(|| SupervisorError::Contract("active worker is absent".to_owned()))?;
        let id = self.internal_id(token, "tools")?;
        let request = json!({"jsonrpc": "2.0", "id": id.to_json_value(), "method": "tools/list"});
        let _previous = self.internal.insert(
            id,
            InternalRequest {
                worker: token,
                action: InternalAction::ActiveCatalog,
            },
        );
        self.send_active(serialize(&request)?).await
    }

    async fn reject_candidate<W>(
        &mut self,
        reason: &str,
        stdout: &mut W,
    ) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        if let Some(candidate) = self.candidate.take() {
            if candidate.purpose == CandidatePurpose::Rollover {
                self.rejected_generation = Some(candidate.worker.release.generation().clone());
            }
            candidate.worker.kill();
            self.internal
                .retain(|_, request| request.worker != candidate.worker.token);
            eprintln!("libmcp: {}: {reason}", self.config.server);
        }
        if self.active.is_some() {
            self.phase = RolloverPhase::None;
            self.refresh_request = None;
            self.flush_queue(stdout).await?;
        }
        Ok(())
    }

    fn classify_request(
        &self,
        frame: &FramedMessage,
        method: &crate::RpcMethod,
    ) -> crate::ReplayContract {
        if method.is_initialize()
            || matches!(
                method.as_str(),
                "ping"
                    | "tools/list"
                    | "resources/list"
                    | "resources/templates/list"
                    | "resources/read"
                    | "prompts/list"
                    | "prompts/get"
                    | "completion/complete"
            )
        {
            return EffectRecovery::ReplaySafe.replay_contract();
        }
        if !method.is_tools_call() {
            return EffectRecovery::AtMostOnce.replay_contract();
        }
        let Some(meta) = parse_tool_call_meta(frame, method) else {
            return EffectRecovery::AtMostOnce.replay_contract();
        };
        let Some(contract) = self
            .published_catalog
            .as_ref()
            .and_then(|catalog| catalog.contract(&meta.tool_name))
        else {
            return EffectRecovery::AtMostOnce.replay_contract();
        };
        if contract.recovery() == EffectRecovery::Deduplicated {
            let key_present = contract
                .deduplication_key_pointer()
                .and_then(|pointer| frame.value().pointer(pointer))
                .is_some_and(|key| key.is_string() || key.is_number());
            if !key_present {
                return EffectRecovery::AtMostOnce.replay_contract();
            }
        }
        match contract.recovery() {
            EffectRecovery::ProbeRequired => EffectRecovery::AtMostOnce.replay_contract(),
            recovery => recovery.replay_contract(),
        }
    }

    fn record_session_transition(&mut self, frame: FramedMessage) -> Result<(), SupervisorError> {
        let Some(meta) = parse_tool_call_meta(&frame, &crate::RpcMethod::tools_call()) else {
            return Ok(());
        };
        let Some(contract) = self
            .published_catalog
            .as_ref()
            .and_then(|catalog| catalog.contract(&meta.tool_name))
        else {
            return Ok(());
        };
        match contract.state() {
            SessionStateContract::Stateless => Ok(()),
            SessionStateContract::Journaled { key } => {
                let key = if key.starts_with('/') {
                    let value = frame.value().pointer(key).ok_or_else(|| {
                        SupervisorError::Contract(format!(
                            "session journal key pointer `{key}` is absent"
                        ))
                    })?;
                    let scalar = value.as_str().map(str::to_owned).or_else(|| {
                        (value.is_number() || value.is_boolean()).then(|| value.to_string())
                    });
                    scalar.ok_or_else(|| {
                        SupervisorError::Contract(format!(
                            "session journal key pointer `{key}` is not scalar"
                        ))
                    })?
                } else {
                    key.clone()
                };
                self.journal
                    .record(&format!("{}:{key}", meta.tool_name.as_str()), frame)
            }
            SessionStateContract::Checkpointed { .. } => {
                self.journal.checkpointed = true;
                Ok(())
            }
            SessionStateContract::GenerationPinned => {
                self.journal.pinned = true;
                Ok(())
            }
        }
    }

    async fn flush_queue<W>(&mut self, stdout: &mut W) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        while self.phase == RolloverPhase::None && self.active.is_some() {
            match self.kernel.pop_next_dispatch()? {
                DispatchQueueOutcome::Replay(frame) => {
                    if !self
                        .send_active_or_recover(frame.payload().to_vec(), stdout)
                        .await?
                    {
                        break;
                    }
                }
                DispatchQueueOutcome::ClientFrame(frame) => {
                    if matches!(
                        frame.classify(),
                        RpcEnvelopeKind::Request { ref method, .. } if method.as_str() == "tools/list"
                    ) {
                        self.reply_catalog(&frame, stdout).await?;
                        continue;
                    }
                    match frame.classify() {
                        RpcEnvelopeKind::Request { ref method, .. } => {
                            let replay = self.classify_request(&frame, method);
                            if let Err(rejection) =
                                self.kernel
                                    .begin_request_dispatch(&frame, replay, PENDING_CAPACITY)
                            {
                                self.reject_client_frame(&frame, rejection, stdout).await?;
                                continue;
                            }
                        }
                        RpcEnvelopeKind::Notification { .. } => {}
                        RpcEnvelopeKind::Response { .. } => {
                            return Err(SupervisorError::Contract(
                                "callback response entered the worker queue".to_owned(),
                            ));
                        }
                    }
                    if !self
                        .send_active_or_recover(frame.payload().to_vec(), stdout)
                        .await?
                    {
                        break;
                    }
                }
                DispatchQueueOutcome::HeldForProbe { .. } | DispatchQueueOutcome::Empty => break,
            }
        }
        Ok(())
    }

    async fn reply_catalog<W>(
        &self,
        request: &FramedMessage,
        stdout: &mut W,
    ) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        let RpcEnvelopeKind::Request { id, .. } = request.classify() else {
            return Err(SupervisorError::Contract(
                "catalog reply target is not a request".to_owned(),
            ));
        };
        let catalog = self
            .published_catalog
            .as_ref()
            .ok_or_else(|| SupervisorError::Contract("public catalog is absent".to_owned()))?;
        let response = json!({
            "jsonrpc": "2.0",
            "id": id.to_json_value(),
            "result": {"tools": catalog.public_tools()}
        });
        write_value(stdout, &response, self.config.frame_limit).await
    }

    async fn route_callback_response<W>(
        &mut self,
        id: &RequestId,
        frame: FramedMessage,
        stdout: &mut W,
    ) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        let Some(route) = self.callbacks.remove(id) else {
            return Ok(());
        };
        let rewritten = rewrite_id(frame.value(), &route.original)?;
        if let Err(error) = self.send_worker(route.worker, serialize(&rewritten)?).await {
            self.handle_worker_loss_by_id(Some(route.worker), &error.to_string(), stdout)
                .await?;
        }
        Ok(())
    }

    async fn route_cancellation<W>(
        &mut self,
        frame: FramedMessage,
        stdout: &mut W,
    ) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        let target = frame
            .value()
            .get("params")
            .and_then(|params| params.get("requestId"))
            .and_then(RequestId::from_json_value);
        let worker = target
            .as_ref()
            .and_then(|id| self.kernel.pending_request(id))
            .and(self.active_token());
        if let Some(token) = worker
            && let Err(error) = self.send_worker(token, frame.payload().to_vec()).await
        {
            self.handle_worker_loss_by_id(Some(token), &error.to_string(), stdout)
                .await?;
        }
        Ok(())
    }

    fn queue(&mut self, frame: FramedMessage) -> Result<(), SupervisorError> {
        self.kernel
            .queue_client_frame(frame, QUEUE_CAPACITY)
            .map_err(Into::into)
    }

    async fn queue_or_reject<W>(
        &mut self,
        frame: FramedMessage,
        stdout: &mut W,
    ) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        let rejection_id = match frame.classify() {
            RpcEnvelopeKind::Request { id, .. } => Some(id),
            RpcEnvelopeKind::Response { .. } | RpcEnvelopeKind::Notification { .. } => None,
        };
        match self.queue(frame) {
            Ok(()) => Ok(()),
            Err(SupervisorError::Session(rejection)) => {
                self.reject_client_id(rejection_id.as_ref(), rejection, stdout)
                    .await
            }
            Err(error) => Err(error),
        }
    }

    async fn reject_client_frame<W>(
        &self,
        frame: &FramedMessage,
        rejection: HostRejection,
        stdout: &mut W,
    ) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        let id = match frame.classify() {
            RpcEnvelopeKind::Request { id, .. } => Some(id),
            RpcEnvelopeKind::Response { .. } | RpcEnvelopeKind::Notification { .. } => None,
        };
        self.reject_client_id(id.as_ref(), rejection, stdout).await
    }

    async fn reject_client_id<W>(
        &self,
        id: Option<&RequestId>,
        rejection: HostRejection,
        stdout: &mut W,
    ) -> Result<(), SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        let Some(id) = id else {
            eprintln!("libmcp: {}: {rejection}", self.config.server);
            return Ok(());
        };
        let response = rpc_error(id, rejection.code(), rejection.message());
        write_value(stdout, &response, self.config.frame_limit).await
    }

    fn gating_new_work(&self) -> bool {
        matches!(
            self.phase,
            RolloverPhase::Draining | RolloverPhase::Recovering
        )
    }

    fn active_token(&self) -> Option<u64> {
        self.active.as_ref().map(|worker| worker.token)
    }

    fn active_callbacks(&self) -> usize {
        let active = self.active_token();
        self.callbacks
            .values()
            .filter(|route| Some(route.worker) == active)
            .count()
    }

    async fn send_active(&self, frame: Vec<u8>) -> Result<(), SupervisorError> {
        let active = self
            .active
            .as_ref()
            .ok_or_else(|| SupervisorError::Contract("active worker is absent".to_owned()))?;
        active.send(frame).await
    }

    async fn send_active_or_recover<W>(
        &mut self,
        frame: Vec<u8>,
        stdout: &mut W,
    ) -> Result<bool, SupervisorError>
    where
        W: AsyncWrite + Unpin,
    {
        let token = self.active_token();
        match self.send_active(frame).await {
            Ok(()) => Ok(true),
            Err(error) => {
                self.handle_worker_loss_by_id(token, &error.to_string(), stdout)
                    .await?;
                Ok(false)
            }
        }
    }

    async fn send_worker(&self, token: u64, frame: Vec<u8>) -> Result<(), SupervisorError> {
        if let Some(active) = self.active.as_ref()
            && active.token == token
        {
            return active.send(frame).await;
        }
        if let Some(candidate) = self.candidate.as_ref()
            && candidate.worker.token == token
        {
            return candidate.worker.send(frame).await;
        }
        Err(SupervisorError::Contract(format!(
            "worker {token} is no longer owned"
        )))
    }

    fn internal_id(&mut self, token: u64, kind: &str) -> Result<RequestId, SupervisorError> {
        self.next_internal = self
            .next_internal
            .checked_add(1)
            .ok_or_else(|| SupervisorError::Contract("internal request ID exhausted".to_owned()))?;
        Ok(RequestId::text(format!(
            "io.libmcp/{token}/{kind}/{}",
            self.next_internal
        )))
    }

    fn next_callback_id(&mut self) -> Result<RequestId, SupervisorError> {
        self.next_callback = self
            .next_callback
            .checked_add(1)
            .ok_or_else(|| SupervisorError::Contract("callback request ID exhausted".to_owned()))?;
        Ok(RequestId::text(format!(
            "io.libmcp/callback/{}",
            self.next_callback
        )))
    }

    fn shutdown(&mut self) {
        if let Some(active) = self.active.take() {
            active.kill();
        }
        if let Some(candidate) = self.candidate.take() {
            candidate.worker.kill();
        }
    }
}

fn spawn_client_reader(events: mpsc::Sender<Event>, limit: FrameLimit) {
    let _reader = tokio::spawn(async move {
        let mut stdin = BufReader::new(tokio::io::stdin());
        loop {
            match crate::read_frame(&mut stdin, limit).await {
                Ok(FrameReadOutcome::Frame(payload)) => {
                    if events.send(Event::ClientFrame(payload)).await.is_err() {
                        return;
                    }
                }
                Ok(FrameReadOutcome::EndOfStream) => {
                    let _sent = events.send(Event::ClientEnd).await;
                    return;
                }
                Err(error) => {
                    let _sent = events.send(Event::ClientFault(error)).await;
                    return;
                }
            }
        }
    });
}

fn spawn_worker_io(
    token: u64,
    mut child: Child,
    mut commands: mpsc::Receiver<WorkerCommand>,
    events: mpsc::Sender<Event>,
    limit: FrameLimit,
) -> Result<(), SupervisorError> {
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| SupervisorError::Contract("worker stdin was not piped".to_owned()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SupervisorError::Contract("worker stdout was not piped".to_owned()))?;
    let _worker = tokio::spawn(async move {
        let detail = worker_io_loop(
            &mut child,
            stdin,
            stdout,
            &mut commands,
            &events,
            token,
            limit,
        )
        .await;
        let _sent = events.send(Event::WorkerGone { token, detail }).await;
    });
    Ok(())
}

async fn worker_io_loop(
    child: &mut Child,
    mut stdin: ChildStdin,
    stdout: tokio::process::ChildStdout,
    commands: &mut mpsc::Receiver<WorkerCommand>,
    events: &mpsc::Sender<Event>,
    token: u64,
    limit: FrameLimit,
) -> String {
    let mut stdout = BufReader::new(stdout);
    loop {
        tokio::select! {
            frame = crate::read_frame(&mut stdout, limit) => match frame {
                Ok(FrameReadOutcome::Frame(payload)) => {
                    if events.send(Event::WorkerFrame { token, payload }).await.is_err() {
                        let _killed = child.kill().await;
                        let _status = child.wait().await;
                        return "host event receiver closed".to_owned();
                    }
                }
                Ok(FrameReadOutcome::EndOfStream) => {
                    let status = child.wait().await;
                    return match status {
                        Ok(status) => format!("worker exited with {status}"),
                        Err(error) => format!("worker stdout closed; wait failed: {error}"),
                    };
                }
                Err(error) => {
                    let _killed = child.kill().await;
                    let _status = child.wait().await;
                    return format!("worker output failed: {error}");
                }
            },
            command = commands.recv() => match command {
                Some(WorkerCommand::Frame(payload)) => {
                    if let Err(error) = write_frame(&mut stdin, &payload, limit).await {
                        let _killed = child.kill().await;
                        let _status = child.wait().await;
                        return format!("worker input failed: {error}");
                    }
                }
                Some(WorkerCommand::Kill) | None => {
                    let _killed = child.kill().await;
                    let _status = child.wait().await;
                    return "worker reaped by host".to_owned();
                }
            }
        }
    }
}

fn parse_catalog_response(frame: &FramedMessage) -> Result<ToolCatalog, SupervisorError> {
    let result = frame
        .value()
        .get("result")
        .ok_or_else(|| SupervisorError::Contract("tools/list response has no result".to_owned()))?;
    ToolCatalog::parse(result).map_err(|error| SupervisorError::Contract(error.to_string()))
}

fn patch_initialize_response(mut response: Value) -> Value {
    let Some(result) = response.get_mut("result").and_then(Value::as_object_mut) else {
        return response;
    };
    let capabilities = result
        .entry("capabilities".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if !capabilities.is_object() {
        *capabilities = Value::Object(Map::new());
    }
    let Some(capabilities) = capabilities.as_object_mut() else {
        return response;
    };
    let tools = capabilities
        .entry("tools".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if !tools.is_object() {
        *tools = Value::Object(Map::new());
    }
    let Some(tools) = tools.as_object_mut() else {
        return response;
    };
    let _previous = tools.insert("listChanged".to_owned(), Value::Bool(true));
    response
}

fn rewrite_id(value: &Value, id: &RequestId) -> Result<Value, SupervisorError> {
    let mut value = value.clone();
    let object = value
        .as_object_mut()
        .ok_or_else(|| SupervisorError::Contract("JSON-RPC frame is not an object".to_owned()))?;
    let _previous = object.insert("id".to_owned(), id.to_json_value());
    Ok(value)
}

fn rpc_error(id: &RequestId, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.to_json_value(),
        "error": {"code": code, "message": message}
    })
}

fn serialize(value: &Value) -> Result<Vec<u8>, SupervisorError> {
    Ok(serde_json::to_vec(value)?)
}

async fn write_value<W>(
    writer: &mut W,
    value: &Value,
    limit: FrameLimit,
) -> Result<(), SupervisorError>
where
    W: AsyncWrite + Unpin,
{
    write_frame(writer, &serialize(value)?, limit).await?;
    Ok(())
}
