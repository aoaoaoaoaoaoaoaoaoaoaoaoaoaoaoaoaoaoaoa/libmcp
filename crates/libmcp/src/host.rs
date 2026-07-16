//! Durable public-session runtime primitives for hardened MCP hosts.

use crate::{
    jsonrpc::{
        FramedMessage, RequestId, RpcEnvelopeKind, RpcMethod, ToolCallMeta, parse_tool_call_meta,
    },
    replay::{
        ExecutionKnowledge, ProbeResolution, ReplayAllowance, ReplayContract, RequestDisposition,
        request_disposition,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant as StdInstant, SystemTime, UNIX_EPOCH},
};

/// Session readiness for worker replay purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    /// No successful `initialize` has been forwarded yet.
    Cold,
    /// The public session is live and must be reseeded after worker churn.
    Live,
}

/// Captured initialize request needed to reseed a replacement worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SeededInitializeRequest {
    /// Original request identifier.
    pub id: RequestId,
    /// Original serialized JSON-RPC frame.
    pub payload: Vec<u8>,
}

/// Captured initialization seed for worker replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InitializationSeed {
    /// Original initialize request.
    pub initialize_request: SeededInitializeRequest,
    /// Best-effort initialized notification payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initialized_notification: Option<Vec<u8>>,
}

/// Common host-side request rejections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostRejection {
    /// Recovery queue capacity was exhausted.
    QueueOverflow,
    /// The request exhausted its automatic replay budget.
    ReplayBudgetExhausted,
    /// An outstanding request already owns this public ID.
    DuplicateRequestId,
    /// The pending invocation capacity was exhausted.
    PendingCapacityExhausted,
    /// The frame was not a JSON-RPC request.
    InvalidRequestFrame,
    /// The request may have executed and its contract forbids replay.
    AmbiguousOutcome,
    /// A response or probe named no pending invocation.
    RequestNotPending,
    /// Kernel state contradicted the execution transition law.
    InvalidExecutionState,
}

impl HostRejection {
    /// JSON-RPC error code for the rejection.
    #[must_use]
    pub const fn code(self) -> i64 {
        match self {
            Self::QueueOverflow => -32097,
            Self::ReplayBudgetExhausted => -32095,
            Self::DuplicateRequestId => -32600,
            Self::PendingCapacityExhausted => -32096,
            Self::InvalidRequestFrame => -32600,
            Self::AmbiguousOutcome => -32094,
            Self::RequestNotPending => -32600,
            Self::InvalidExecutionState => -32603,
        }
    }

    /// Human-facing rejection message.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::QueueOverflow => "worker queue overflow during recovery",
            Self::ReplayBudgetExhausted => "worker restart replay budget exhausted for request",
            Self::DuplicateRequestId => "public request id is already outstanding",
            Self::PendingCapacityExhausted => "pending request capacity exhausted",
            Self::InvalidRequestFrame => "frame is not a JSON-RPC request",
            Self::AmbiguousOutcome => "request outcome is ambiguous and replay is forbidden",
            Self::RequestNotPending => "request id is not pending",
            Self::InvalidExecutionState => "request execution state transition is invalid",
        }
    }
}

/// Live pending request tracked by the host.
#[derive(Debug, Clone)]
pub struct PendingRequest {
    method: RpcMethod,
    sequence: u64,
    frame: FramedMessage,
    replay_contract: ReplayContract,
    started_at: StdInstant,
    tool_call_meta: Option<ToolCallMeta>,
    execution_knowledge: ExecutionKnowledge,
    replay_attempts: u8,
    scheduled_disposition: Option<RequestDisposition>,
}

impl PendingRequest {
    /// Returns the immutable public JSON-RPC method.
    #[must_use]
    pub const fn method(&self) -> &RpcMethod {
        &self.method
    }

    /// Returns the stable ordering sequence.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Returns the immutable original request frame.
    #[must_use]
    pub const fn frame(&self) -> &FramedMessage {
        &self.frame
    }

    /// Returns the invocation's replay contract.
    #[must_use]
    pub const fn replay_contract(&self) -> ReplayContract {
        self.replay_contract
    }

    /// Returns the local invocation start time.
    #[must_use]
    pub const fn started_at(&self) -> StdInstant {
        self.started_at
    }

    /// Returns best-effort tool metadata for telemetry grouping.
    #[must_use]
    pub const fn tool_call_meta(&self) -> Option<&ToolCallMeta> {
        self.tool_call_meta.as_ref()
    }

    /// Returns what the kernel knows about execution.
    #[must_use]
    pub const fn execution_knowledge(&self) -> ExecutionKnowledge {
        self.execution_knowledge
    }

    /// Returns replay attempts actually dispatched.
    #[must_use]
    pub const fn replay_attempts(&self) -> u8 {
        self.replay_attempts
    }
}

/// Pending request plus the number of replay attempts consumed so far.
#[derive(Debug, Clone)]
pub struct CompletedPendingRequest {
    /// Pending request metadata and original frame.
    pub request: PendingRequest,
    /// Replay attempts consumed for this request.
    pub replay_attempts: u8,
}

/// Result of taking the next recovery-ordered dispatch.
#[derive(Debug, Clone)]
pub enum DispatchQueueOutcome {
    /// One frame has crossed the kernel's dispatch boundary.
    Frame(FramedMessage),
    /// An older invocation blocks the queue pending consumer evidence.
    HeldForProbe {
        /// Public request identifier awaiting a probe.
        request_id: RequestId,
    },
    /// No frame is waiting.
    Empty,
}

/// Result of applying consumer probe evidence.
#[derive(Debug, Clone)]
pub enum ProbeResolutionOutcome {
    /// The prior attempt completed and the invocation left pending state.
    Completed(Box<CompletedPendingRequest>),
    /// The held invocation is now authorized to replay in order.
    ReplayAuthorized {
        /// Public request identifier.
        request_id: RequestId,
    },
}

/// Recovery-time configuration for pending-request replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayBudget {
    /// Maximum replay attempts per request.
    pub max_attempts: u8,
    /// Total queue capacity, including replayed and newly queued requests.
    pub queue_capacity: usize,
}

/// Request dropped during replay requeue.
#[derive(Debug, Clone)]
pub struct RejectedReplay {
    /// Request identifier.
    pub request_id: RequestId,
    /// Pending request metadata.
    pub request: PendingRequest,
    /// Attempt number that triggered the drop.
    pub next_attempt: Option<u8>,
    /// Rejection reason.
    pub reason: HostRejection,
}

/// Result of rebuilding the replay queue after worker failure.
#[derive(Debug, Clone, Default)]
pub struct ReplayRequeueOutcome {
    /// Requests dropped during recovery.
    pub rejected: Vec<RejectedReplay>,
    /// Requests held in recovery order until consumer probes resolve them.
    pub held_for_probe: Vec<RequestId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingRequestSnapshot {
    request_id: RequestId,
    method: RpcMethod,
    sequence: u64,
    frame: Vec<u8>,
    replay_contract: ReplayContract,
    execution_knowledge: ExecutionKnowledge,
    replay_attempts: u8,
    scheduled_disposition: Option<RequestDisposition>,
    age_ms: u64,
}

/// Serializable kernel snapshot for host self-reexec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostSessionKernelSnapshot {
    /// Current public session phase.
    pub session_phase: SessionPhase,
    /// Captured initialize state, if any.
    pub initialization_seed: Option<InitializationSeed>,
    /// Live pending requests.
    pending: Vec<PendingRequestSnapshot>,
    /// Backlog of client frames waiting on worker readiness.
    pub queued_frames: Vec<Vec<u8>>,
    /// Next pending sequence number.
    pub next_pending_sequence: u64,
}

/// Restored host-session kernel state ready to hydrate a live runtime.
#[derive(Debug, Clone)]
pub struct RestoredHostSessionKernel {
    /// Current public session phase.
    pub session_phase: SessionPhase,
    /// Captured initialize state, if any.
    pub initialization_seed: Option<InitializationSeed>,
    /// Live pending requests.
    pub pending: HashMap<RequestId, PendingRequest>,
    /// Backlog of client frames waiting on worker readiness.
    pub queued_frames: VecDeque<FramedMessage>,
    /// Next pending sequence number.
    pub next_pending_sequence: u64,
}

impl RestoredHostSessionKernel {
    /// Returns an empty cold host-session state.
    #[must_use]
    pub fn cold() -> Self {
        Self {
            session_phase: SessionPhase::Cold,
            initialization_seed: None,
            pending: HashMap::new(),
            queued_frames: VecDeque::new(),
            next_pending_sequence: 0,
        }
    }
}

/// Durable public-session kernel shared by hardened MCP hosts.
#[derive(Debug, Clone)]
pub struct HostSessionKernel {
    session_phase: SessionPhase,
    initialization_seed: Option<InitializationSeed>,
    pending: HashMap<RequestId, PendingRequest>,
    queued_frames: VecDeque<FramedMessage>,
    next_pending_sequence: u64,
}

impl HostSessionKernel {
    /// Constructs a cold kernel.
    #[must_use]
    pub fn cold() -> Self {
        Self::from_restored(RestoredHostSessionKernel::cold())
    }

    /// Hydrates the kernel from restored state.
    #[must_use]
    pub fn from_restored(restored: RestoredHostSessionKernel) -> Self {
        Self {
            session_phase: restored.session_phase,
            initialization_seed: restored.initialization_seed,
            pending: restored.pending,
            queued_frames: restored.queued_frames,
            next_pending_sequence: restored.next_pending_sequence,
        }
    }

    /// Serializes the current kernel state for self-reexec.
    #[must_use]
    pub fn snapshot(&self) -> HostSessionKernelSnapshot {
        let initialization_seed = self.initialization_seed.clone();
        let pending = self
            .pending
            .iter()
            .map(|(request_id, request)| PendingRequestSnapshot {
                request_id: request_id.clone(),
                method: request.method.clone(),
                sequence: request.sequence,
                frame: request.frame.payload().to_vec(),
                replay_contract: request.replay_contract,
                execution_knowledge: request.execution_knowledge,
                replay_attempts: request.replay_attempts,
                scheduled_disposition: request.scheduled_disposition,
                age_ms: duration_millis_u64(request.started_at.elapsed()),
            })
            .collect::<Vec<_>>();
        let queued_frames = self
            .queued_frames
            .iter()
            .map(|frame| frame.payload().to_vec())
            .collect::<Vec<_>>();
        HostSessionKernelSnapshot {
            session_phase: self.session_phase,
            initialization_seed,
            pending,
            queued_frames,
            next_pending_sequence: self.next_pending_sequence,
        }
    }

    /// Returns the current public session phase.
    #[must_use]
    pub const fn session_phase(&self) -> SessionPhase {
        self.session_phase
    }

    /// Returns the captured initialize seed, if any.
    #[must_use]
    pub fn initialization_seed(&self) -> Option<&InitializationSeed> {
        self.initialization_seed.as_ref()
    }

    /// Returns the worker replay seed, synthesizing `initialized` if needed.
    #[must_use]
    pub fn replay_seed(&self) -> Option<InitializationSeed> {
        prepare_replay_seed(self.session_phase, self.initialization_seed.as_ref())
    }

    /// Observes a client frame before it is forwarded or queued.
    pub fn observe_client_frame(&mut self, frame: &FramedMessage) {
        match frame.classify() {
            RpcEnvelopeKind::Request { id, method } if method.is_initialize() => {
                let prior_initialized = self
                    .initialization_seed
                    .as_ref()
                    .and_then(|seed| seed.initialized_notification.clone());
                self.initialization_seed = Some(InitializationSeed {
                    initialize_request: SeededInitializeRequest {
                        id,
                        payload: frame.payload().to_vec(),
                    },
                    initialized_notification: prior_initialized,
                });
            }
            RpcEnvelopeKind::Notification { method } if method.is_initialized_notification() => {
                if let Some(seed) = self.initialization_seed.as_mut() {
                    seed.initialized_notification = Some(frame.payload().to_vec());
                }
            }
            RpcEnvelopeKind::Request { .. }
            | RpcEnvelopeKind::Notification { .. }
            | RpcEnvelopeKind::Response { .. } => {}
        }
    }

    /// Queues a client frame while no ready worker is available.
    pub fn queue_client_frame(
        &mut self,
        frame: FramedMessage,
        queue_capacity: usize,
    ) -> Result<(), HostRejection> {
        if self.queued_frames.len() >= queue_capacity {
            return Err(HostRejection::QueueOverflow);
        }
        if let RpcEnvelopeKind::Request { id, .. } = frame.classify()
            && (self.pending.contains_key(&id) || self.queued_request_id(&id))
        {
            return Err(HostRejection::DuplicateRequestId);
        }
        self.queued_frames.push_back(frame);
        Ok(())
    }

    /// Takes the next dispatch in recovery order.
    ///
    /// Taking a replay crosses the kernel dispatch boundary and consumes its
    /// attempt. A held probe blocks all younger work.
    pub fn pop_next_dispatch(&mut self) -> Result<DispatchQueueOutcome, HostRejection> {
        let Some(frame) = self.queued_frames.front().cloned() else {
            return Ok(DispatchQueueOutcome::Empty);
        };
        let RpcEnvelopeKind::Request { id, .. } = frame.classify() else {
            let _removed = self.queued_frames.pop_front();
            return Ok(DispatchQueueOutcome::Frame(frame));
        };
        let Some(request) = self.pending.get_mut(&id) else {
            let _removed = self.queued_frames.pop_front();
            return Ok(DispatchQueueOutcome::Frame(frame));
        };

        match request.scheduled_disposition {
            Some(RequestDisposition::HoldForProbe) => {
                Ok(DispatchQueueOutcome::HeldForProbe { request_id: id })
            }
            Some(RequestDisposition::Replay) => {
                request.execution_knowledge = request
                    .execution_knowledge
                    .after_dispatch(RequestDisposition::Replay)
                    .map_err(|_| HostRejection::InvalidExecutionState)?;
                request.replay_attempts = request
                    .replay_attempts
                    .checked_add(1)
                    .ok_or(HostRejection::ReplayBudgetExhausted)?;
                request.scheduled_disposition = None;
                let _removed = self.queued_frames.pop_front();
                Ok(DispatchQueueOutcome::Frame(frame))
            }
            Some(
                RequestDisposition::FirstDispatch
                | RequestDisposition::AwaitTerminal
                | RequestDisposition::Completed
                | RequestDisposition::CompleteFromProbe
                | RequestDisposition::RejectAmbiguousOutcome
                | RequestDisposition::RejectReplayExhausted
                | RequestDisposition::RejectUnexpectedProbeResolution,
            )
            | None => Err(HostRejection::InvalidExecutionState),
        }
    }

    /// Returns the number of queued client frames.
    #[must_use]
    pub fn queued_len(&self) -> usize {
        self.queued_frames.len()
    }

    /// Returns whether the queue is empty.
    #[must_use]
    pub fn queue_is_empty(&self) -> bool {
        self.queued_frames.is_empty()
    }

    /// Begins first dispatch of a routed client request.
    ///
    /// Consumers call this immediately before crossing the worker transport
    /// boundary so any subsequent uncertainty is conservatively in flight.
    pub fn begin_request_dispatch(
        &mut self,
        frame: &FramedMessage,
        replay_contract: ReplayContract,
        pending_capacity: usize,
    ) -> Result<RequestId, HostRejection> {
        let RpcEnvelopeKind::Request { id, method } = frame.classify() else {
            return Err(HostRejection::InvalidRequestFrame);
        };
        if self.pending.contains_key(&id) || self.queued_request_id(&id) {
            return Err(HostRejection::DuplicateRequestId);
        }
        if self.pending.len() >= pending_capacity {
            return Err(HostRejection::PendingCapacityExhausted);
        }
        let sequence = self.next_pending_sequence;
        self.next_pending_sequence = self
            .next_pending_sequence
            .checked_add(1)
            .ok_or(HostRejection::InvalidExecutionState)?;
        let request = PendingRequest {
            tool_call_meta: parse_tool_call_meta(frame, &method),
            method,
            sequence,
            frame: frame.clone(),
            replay_contract,
            started_at: StdInstant::now(),
            execution_knowledge: ExecutionKnowledge::InFlight,
            replay_attempts: 0,
            scheduled_disposition: None,
        };
        let _previous = self.pending.insert(id.clone(), request);
        Ok(id)
    }

    /// Records and removes the single terminal outcome for an invocation.
    pub fn complete_request(
        &mut self,
        request_id: &RequestId,
    ) -> Result<CompletedPendingRequest, HostRejection> {
        let pending = self
            .pending
            .get_mut(request_id)
            .ok_or(HostRejection::RequestNotPending)?;
        pending.execution_knowledge = pending
            .execution_knowledge
            .after_terminal_outcome()
            .map_err(|_| HostRejection::InvalidExecutionState)?;
        let request = self
            .pending
            .remove(request_id)
            .ok_or(HostRejection::InvalidExecutionState)?;
        if request.method.is_initialize() {
            self.session_phase = SessionPhase::Live;
        }
        self.remove_queued_request(request_id);
        let replay_attempts = request.replay_attempts;
        Ok(CompletedPendingRequest {
            request,
            replay_attempts,
        })
    }

    /// Rebuilds the replay queue after worker failure.
    pub fn requeue_pending_for_replay(&mut self, budget: ReplayBudget) -> ReplayRequeueOutcome {
        for request in self.pending.values_mut() {
            request.execution_knowledge = request.execution_knowledge.after_worker_loss();
            request.scheduled_disposition = None;
        }
        let mut ordered_pending = self
            .pending
            .iter()
            .map(|(id, request)| (id.clone(), request.clone()))
            .collect::<Vec<_>>();
        ordered_pending.sort_by_key(|(_, request)| request.sequence);
        let pending_ids = self.pending.keys().cloned().collect::<HashSet<_>>();

        let mut retained_queue = VecDeque::<FramedMessage>::new();
        while let Some(frame) = self.queued_frames.pop_front() {
            let should_drop = match frame.classify() {
                RpcEnvelopeKind::Request { id, .. } => pending_ids.contains(&id),
                RpcEnvelopeKind::Notification { .. } | RpcEnvelopeKind::Response { .. } => false,
            };
            if !should_drop {
                retained_queue.push_back(frame);
            }
        }

        let mut replay_frames = VecDeque::<FramedMessage>::new();
        let mut dropped_ids = Vec::<RequestId>::new();
        let mut rejected = Vec::<RejectedReplay>::new();
        let mut held_for_probe = Vec::<RequestId>::new();

        for (request_id, request) in ordered_pending {
            let allowance = ReplayAllowance::new(request.replay_attempts, budget.max_attempts);
            let disposition = request_disposition(
                request.execution_knowledge,
                request.replay_contract,
                None,
                allowance,
            );
            let reason = match disposition {
                RequestDisposition::RejectReplayExhausted => {
                    Some(HostRejection::ReplayBudgetExhausted)
                }
                RequestDisposition::RejectAmbiguousOutcome => Some(HostRejection::AmbiguousOutcome),
                RequestDisposition::Replay | RequestDisposition::HoldForProbe => None,
                RequestDisposition::FirstDispatch
                | RequestDisposition::AwaitTerminal
                | RequestDisposition::Completed
                | RequestDisposition::CompleteFromProbe
                | RequestDisposition::RejectUnexpectedProbeResolution => {
                    Some(HostRejection::InvalidExecutionState)
                }
            };
            if let Some(reason) = reason {
                dropped_ids.push(request_id.clone());
                rejected.push(RejectedReplay {
                    request_id,
                    request,
                    next_attempt: allowance.next_attempt(),
                    reason,
                });
                continue;
            }

            if replay_frames.len().saturating_add(retained_queue.len()) >= budget.queue_capacity {
                dropped_ids.push(request_id.clone());
                rejected.push(RejectedReplay {
                    request_id,
                    request,
                    next_attempt: allowance.next_attempt(),
                    reason: HostRejection::QueueOverflow,
                });
                continue;
            }

            if let Some(pending) = self.pending.get_mut(&request_id) {
                pending.scheduled_disposition = Some(disposition);
            }
            if disposition == RequestDisposition::HoldForProbe {
                held_for_probe.push(request_id);
            }
            replay_frames.push_back(request.frame.clone());
        }

        for request_id in dropped_ids {
            let _removed = self.pending.remove(&request_id);
        }

        replay_frames.append(&mut retained_queue);
        self.queued_frames = replay_frames;
        ReplayRequeueOutcome {
            rejected,
            held_for_probe,
        }
    }

    /// Applies explicit consumer evidence to a held probe-required request.
    pub fn resolve_probe(
        &mut self,
        request_id: &RequestId,
        resolution: ProbeResolution,
        max_attempts: u8,
    ) -> Result<ProbeResolutionOutcome, HostRejection> {
        let pending = self
            .pending
            .get(request_id)
            .ok_or(HostRejection::RequestNotPending)?;
        if pending.scheduled_disposition != Some(RequestDisposition::HoldForProbe) {
            return Err(HostRejection::InvalidExecutionState);
        }
        let disposition = request_disposition(
            pending.execution_knowledge,
            pending.replay_contract,
            Some(resolution),
            ReplayAllowance::new(pending.replay_attempts, max_attempts),
        );
        match disposition {
            RequestDisposition::Replay => {
                let pending = self
                    .pending
                    .get_mut(request_id)
                    .ok_or(HostRejection::InvalidExecutionState)?;
                pending.scheduled_disposition = Some(RequestDisposition::Replay);
                Ok(ProbeResolutionOutcome::ReplayAuthorized {
                    request_id: request_id.clone(),
                })
            }
            RequestDisposition::CompleteFromProbe => {
                let pending = self
                    .pending
                    .get_mut(request_id)
                    .ok_or(HostRejection::InvalidExecutionState)?;
                pending.execution_knowledge = pending
                    .execution_knowledge
                    .after_completed_probe()
                    .map_err(|_| HostRejection::InvalidExecutionState)?;
                self.remove_queued_request(request_id);
                let request = self
                    .pending
                    .remove(request_id)
                    .ok_or(HostRejection::InvalidExecutionState)?;
                let replay_attempts = request.replay_attempts;
                Ok(ProbeResolutionOutcome::Completed(Box::new(
                    CompletedPendingRequest {
                        request,
                        replay_attempts,
                    },
                )))
            }
            RequestDisposition::RejectReplayExhausted => {
                self.remove_queued_request(request_id);
                let _removed = self.pending.remove(request_id);
                Err(HostRejection::ReplayBudgetExhausted)
            }
            RequestDisposition::FirstDispatch
            | RequestDisposition::AwaitTerminal
            | RequestDisposition::Completed
            | RequestDisposition::HoldForProbe
            | RequestDisposition::RejectAmbiguousOutcome
            | RequestDisposition::RejectUnexpectedProbeResolution => {
                Err(HostRejection::InvalidExecutionState)
            }
        }
    }

    fn queued_request_id(&self, request_id: &RequestId) -> bool {
        self.queued_frames.iter().any(|frame| {
            matches!(frame.classify(), RpcEnvelopeKind::Request { id, .. } if &id == request_id)
        })
    }

    fn remove_queued_request(&mut self, request_id: &RequestId) {
        self.queued_frames.retain(|frame| {
            !matches!(frame.classify(), RpcEnvelopeKind::Request { id, .. } if &id == request_id)
        });
    }
}

impl HostSessionKernelSnapshot {
    /// Restores a live host-session state from the serialized snapshot.
    pub fn restore(self) -> io::Result<RestoredHostSessionKernel> {
        let now = StdInstant::now();
        let mut pending = HashMap::with_capacity(self.pending.len());
        for snapshot in self.pending {
            let frame = FramedMessage::parse(snapshot.frame)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let started_at = now
                .checked_sub(Duration::from_millis(snapshot.age_ms))
                .unwrap_or(now);
            let tool_call_meta = parse_tool_call_meta(&frame, &snapshot.method);
            let previous = pending.insert(
                snapshot.request_id,
                PendingRequest {
                    method: snapshot.method,
                    sequence: snapshot.sequence,
                    frame,
                    replay_contract: snapshot.replay_contract,
                    started_at,
                    tool_call_meta,
                    execution_knowledge: snapshot.execution_knowledge,
                    replay_attempts: snapshot.replay_attempts,
                    scheduled_disposition: snapshot.scheduled_disposition,
                },
            );
            if previous.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "duplicate request id in host reexec snapshot",
                ));
            }
        }

        let mut queued_frames = VecDeque::with_capacity(self.queued_frames.len());
        for payload in self.queued_frames {
            queued_frames.push_back(
                FramedMessage::parse(payload)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            );
        }

        Ok(RestoredHostSessionKernel {
            session_phase: self.session_phase,
            initialization_seed: self.initialization_seed,
            pending,
            queued_frames,
            next_pending_sequence: self.next_pending_sequence,
        })
    }
}

/// Returns the synthesized initialized notification used when only the request seed survived.
#[must_use]
pub fn synthesized_initialized_notification() -> Vec<u8> {
    br#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#.to_vec()
}

/// Prepares a replay seed based on the current session phase.
#[must_use]
pub fn prepare_replay_seed(
    session_phase: SessionPhase,
    initialization_seed: Option<&InitializationSeed>,
) -> Option<InitializationSeed> {
    match session_phase {
        SessionPhase::Cold => None,
        SessionPhase::Live => initialization_seed.cloned().map(|mut seed| {
            if seed.initialized_notification.is_none() {
                seed.initialized_notification = Some(synthesized_initialized_notification());
            }
            seed
        }),
    }
}

/// Computes a temporary snapshot path for host self-reexec state.
#[must_use]
pub fn snapshot_temp_path(prefix: &str) -> PathBuf {
    let stamp = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis(),
        Err(_) => 0,
    };
    std::env::temp_dir().join(format!("{prefix}-{}-{stamp}.json", std::process::id()))
}

/// Serializes a snapshot to a temporary file for a later exec handoff.
pub fn write_snapshot_file<T>(prefix: &str, snapshot: &T) -> io::Result<PathBuf>
where
    T: Serialize,
{
    let serialized = serde_json::to_vec(snapshot).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to serialize host reexec snapshot: {error}"),
        )
    })?;
    let path = snapshot_temp_path(prefix);
    fs::write(&path, serialized)?;
    Ok(path)
}

/// Loads and deletes a snapshot file referenced by an environment variable.
pub fn load_snapshot_file_from_env<T>(env_var: &str) -> io::Result<Option<T>>
where
    T: DeserializeOwned,
{
    let raw_path = std::env::var_os(env_var);
    let Some(raw_path) = raw_path.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(raw_path);
    let serialized = fs::read(&path)?;
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    serde_json::from_slice(&serialized)
        .map(Some)
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to decode host reexec snapshot: {error}"),
            )
        })
}

/// Best-effort cleanup for a temporary snapshot file.
pub fn remove_snapshot_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        DispatchQueueOutcome, FramedMessage, HostRejection, HostSessionKernel,
        HostSessionKernelSnapshot, InitializationSeed, PendingRequest, ProbeResolutionOutcome,
        ReplayBudget, RequestId, RpcMethod, SeededInitializeRequest, SessionPhase,
        prepare_replay_seed, synthesized_initialized_notification,
    };
    use crate::{ExecutionKnowledge, ProbeResolution, ReplayContract};
    use serde_json::json;

    #[test]
    fn prepare_replay_seed_synthesizes_initialized_notification_when_missing() {
        let seed = InitializationSeed {
            initialize_request: SeededInitializeRequest {
                id: RequestId::number(1),
                payload: br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_vec(),
            },
            initialized_notification: None,
        };

        let prepared = prepare_replay_seed(SessionPhase::Live, Some(&seed));
        assert!(prepared.is_some(), "expected live session replay seed");
        let prepared = match prepared {
            Some(value) => value,
            None => return,
        };
        assert_eq!(
            prepared.initialized_notification,
            Some(synthesized_initialized_notification())
        );
    }

    #[test]
    fn host_session_kernel_snapshot_roundtrip_restores_pending_and_queue() {
        let pending_payload = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "diagnostics",
                "arguments": {
                    "file_path": "/tmp/lib.rs"
                }
            }
        }));
        assert!(pending_payload.is_ok());
        let pending_payload = match pending_payload {
            Ok(value) => value,
            Err(_) => return,
        };

        let queued_payload = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "tools/call",
            "params": {
                "name": "health_snapshot",
                "arguments": {}
            }
        }));
        assert!(queued_payload.is_ok());
        let queued_payload = match queued_payload {
            Ok(value) => value,
            Err(_) => return,
        };

        let snapshot = HostSessionKernelSnapshot {
            session_phase: SessionPhase::Live,
            initialization_seed: Some(InitializationSeed {
                initialize_request: SeededInitializeRequest {
                    id: RequestId::number(1),
                    payload: br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#
                        .to_vec(),
                },
                initialized_notification: None,
            }),
            pending: vec![super::PendingRequestSnapshot {
                request_id: RequestId::number(7),
                method: RpcMethod::tools_call(),
                sequence: 3,
                frame: pending_payload,
                replay_contract: ReplayContract::Convergent,
                execution_knowledge: ExecutionKnowledge::OutcomeUnknown,
                replay_attempts: 2,
                scheduled_disposition: None,
                age_ms: 25,
            }],
            queued_frames: vec![queued_payload],
            next_pending_sequence: 9,
        };

        let restored = snapshot.restore();
        assert!(
            restored.is_ok(),
            "expected restore to succeed: {restored:?}"
        );
        let restored = match restored {
            Ok(value) => value,
            Err(_) => return,
        };

        assert_eq!(restored.session_phase, SessionPhase::Live);
        assert_eq!(restored.next_pending_sequence, 9);
        assert_eq!(restored.pending.len(), 1);
        assert_eq!(restored.queued_frames.len(), 1);
        let pending = restored.pending.get(&RequestId::number(7));
        assert!(pending.is_some(), "expected pending request to round-trip");
        let pending = match pending {
            Some(value) => value,
            None => return,
        };
        assert_eq!(pending.sequence, 3);
        assert!(pending.method.is_tools_call());
        assert_eq!(pending.replay_contract, ReplayContract::Convergent);
        assert_eq!(pending.replay_attempts(), 2);
        assert_eq!(
            pending.execution_knowledge(),
            ExecutionKnowledge::OutcomeUnknown
        );
        assert!(
            pending.tool_call_meta.is_some(),
            "expected tool metadata to be reconstructed from replay snapshot"
        );
    }

    #[test]
    fn replay_requeue_drops_requests_that_exhaust_budget() {
        let diagnostics = FramedMessage::parse(
            br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"diagnostics","arguments":{"file_path":"/tmp/lib.rs"}}}"#.to_vec(),
        );
        assert!(diagnostics.is_ok());
        let diagnostics = match diagnostics {
            Ok(value) => value,
            Err(_) => return,
        };

        let mut kernel = HostSessionKernel::cold();
        let dispatched = kernel.begin_request_dispatch(&diagnostics, ReplayContract::Convergent, 8);
        assert!(dispatched.is_ok());
        let first_recovery = kernel.requeue_pending_for_replay(ReplayBudget {
            max_attempts: 1,
            queue_capacity: 8,
        });
        assert!(first_recovery.rejected.is_empty());
        let replay = kernel.pop_next_dispatch();
        assert!(matches!(replay, Ok(DispatchQueueOutcome::Frame(_))));

        let outcome = kernel.requeue_pending_for_replay(ReplayBudget {
            max_attempts: 1,
            queue_capacity: 8,
        });
        assert_eq!(outcome.rejected.len(), 1);
        assert_eq!(
            outcome.rejected[0].reason,
            HostRejection::ReplayBudgetExhausted
        );
        assert!(kernel.queue_is_empty());
    }

    #[test]
    fn admission_rejects_duplicate_ids_and_bounds_pending_work() {
        let Some(first) = tool_request(11, "first") else {
            return;
        };
        let Some(second) = tool_request(12, "second") else {
            return;
        };
        let mut kernel = HostSessionKernel::cold();

        assert!(
            kernel
                .begin_request_dispatch(&first, ReplayContract::Convergent, 1)
                .is_ok()
        );
        assert_eq!(
            kernel.begin_request_dispatch(&first, ReplayContract::Convergent, 1),
            Err(HostRejection::DuplicateRequestId)
        );
        assert_eq!(
            kernel.begin_request_dispatch(&second, ReplayContract::Convergent, 1),
            Err(HostRejection::PendingCapacityExhausted)
        );

        assert!(kernel.complete_request(&RequestId::number(11)).is_ok());
        assert!(matches!(
            kernel.complete_request(&RequestId::number(11)),
            Err(HostRejection::RequestNotPending)
        ));
        assert!(
            kernel
                .begin_request_dispatch(&first, ReplayContract::Convergent, 1)
                .is_ok()
        );
    }

    #[test]
    fn recovery_obeys_contracts_and_blocks_younger_work_at_probes() {
        let Some(convergent) = tool_request(21, "convergent") else {
            return;
        };
        let Some(probed) = tool_request(22, "probed") else {
            return;
        };
        let Some(forbidden) = tool_request(23, "forbidden") else {
            return;
        };
        let Some(younger) = tool_request(24, "younger") else {
            return;
        };
        let mut kernel = HostSessionKernel::cold();
        assert!(
            kernel
                .begin_request_dispatch(&convergent, ReplayContract::Convergent, 8)
                .is_ok()
        );
        assert!(
            kernel
                .begin_request_dispatch(&probed, ReplayContract::ProbeRequired, 8)
                .is_ok()
        );
        assert!(
            kernel
                .begin_request_dispatch(&forbidden, ReplayContract::NeverReplay, 8)
                .is_ok()
        );
        assert!(kernel.queue_client_frame(younger, 8).is_ok());

        let outcome = kernel.requeue_pending_for_replay(ReplayBudget {
            max_attempts: 2,
            queue_capacity: 8,
        });
        assert_eq!(outcome.held_for_probe, vec![RequestId::number(22)]);
        assert!(matches!(
            outcome.rejected.as_slice(),
            [rejected] if rejected.request_id == RequestId::number(23)
                && rejected.reason == HostRejection::AmbiguousOutcome
        ));
        let convergent_pending = kernel.pending.get(&RequestId::number(21));
        assert!(matches!(convergent_pending, Some(request) if request.replay_attempts() == 0));

        assert!(matches!(
            kernel.pop_next_dispatch(),
            Ok(DispatchQueueOutcome::Frame(frame))
                if matches!(frame.classify(), crate::RpcEnvelopeKind::Request { id, .. } if id == RequestId::number(21))
        ));
        assert!(matches!(
            kernel.pop_next_dispatch(),
            Ok(DispatchQueueOutcome::HeldForProbe { request_id })
                if request_id == RequestId::number(22)
        ));
        let resolved =
            kernel.resolve_probe(&RequestId::number(22), ProbeResolution::SafeToReplay, 2);
        assert!(matches!(
            resolved,
            Ok(ProbeResolutionOutcome::ReplayAuthorized { request_id })
                if request_id == RequestId::number(22)
        ));
        assert!(matches!(
            kernel.pop_next_dispatch(),
            Ok(DispatchQueueOutcome::Frame(frame))
                if matches!(frame.classify(), crate::RpcEnvelopeKind::Request { id, .. } if id == RequestId::number(22))
        ));
        assert!(matches!(
            kernel.pop_next_dispatch(),
            Ok(DispatchQueueOutcome::Frame(frame))
                if matches!(frame.classify(), crate::RpcEnvelopeKind::Request { id, .. } if id == RequestId::number(24))
        ));
        assert!(!kernel.pending.contains_key(&RequestId::number(23)));
        assert!(matches!(
            kernel
                .pending
                .get(&RequestId::number(22))
                .map(PendingRequest::execution_knowledge),
            Some(ExecutionKnowledge::InFlight)
        ));
    }

    #[test]
    fn probe_completion_and_late_terminal_outcomes_cannot_leak_replays() {
        let Some(probed) = tool_request(31, "probed") else {
            return;
        };
        let Some(convergent) = tool_request(32, "convergent") else {
            return;
        };
        let mut kernel = HostSessionKernel::cold();
        assert!(
            kernel
                .begin_request_dispatch(&probed, ReplayContract::ProbeRequired, 8)
                .is_ok()
        );
        assert!(
            kernel
                .begin_request_dispatch(&convergent, ReplayContract::Convergent, 8)
                .is_ok()
        );
        let outcome = kernel.requeue_pending_for_replay(ReplayBudget {
            max_attempts: 1,
            queue_capacity: 8,
        });
        assert_eq!(outcome.held_for_probe.len(), 1);

        let resolved =
            kernel.resolve_probe(&RequestId::number(31), ProbeResolution::AlreadyCompleted, 1);
        assert!(matches!(resolved, Ok(ProbeResolutionOutcome::Completed(_))));
        assert!(matches!(
            kernel.pop_next_dispatch(),
            Ok(DispatchQueueOutcome::Frame(frame))
                if matches!(frame.classify(), crate::RpcEnvelopeKind::Request { id, .. } if id == RequestId::number(32))
        ));

        let second_loss = kernel.requeue_pending_for_replay(ReplayBudget {
            max_attempts: 2,
            queue_capacity: 8,
        });
        assert!(second_loss.rejected.is_empty());
        assert!(!kernel.queue_is_empty());
        assert!(kernel.complete_request(&RequestId::number(32)).is_ok());
        assert!(kernel.queue_is_empty());
        assert!(matches!(
            kernel.complete_request(&RequestId::number(32)),
            Err(HostRejection::RequestNotPending)
        ));
    }

    fn tool_request(id: u64, name: &str) -> Option<FramedMessage> {
        let payload = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": {}}
        }))
        .ok()?;
        FramedMessage::parse(payload).ok()
    }
}
