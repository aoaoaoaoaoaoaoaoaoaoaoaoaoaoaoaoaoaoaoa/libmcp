//! Durable public-session runtime primitives for hardened MCP hosts.

use crate::{
    jsonrpc::{
        FrameParseError, FramedMessage, RequestId, RpcEnvelopeKind, RpcMethod, ToolCallMeta,
        parse_tool_call_meta,
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
    fs::{self, File},
    io::{self, Read as _, Write as _},
    path::Path,
    time::{Duration, Instant as StdInstant},
};
use tempfile::{Builder as TempfileBuilder, TempPath};
use thiserror::Error;

/// Exact snapshot format understood by this release line.
pub const SNAPSHOT_FORMAT_VERSION: u16 = 1;

/// Public MCP initialization phase, independent of worker readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    /// No initialize request is active.
    Cold,
    /// The public initialize request is in flight.
    Initializing,
    /// Initialize succeeded; the client notification has not yet arrived.
    AwaitingInitialized,
    /// The client completed the public initialization handshake.
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

impl std::fmt::Display for HostRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for HostRejection {}

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
    format_version: u16,
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

/// Explicit bounds applied before snapshot state is hydrated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotLimits {
    max_pending: usize,
    max_queued: usize,
    max_frame_bytes: usize,
    max_replay_attempts: u8,
}

impl SnapshotLimits {
    /// Constructs snapshot restoration limits.
    pub fn try_new(
        max_pending: usize,
        max_queued: usize,
        max_frame_bytes: usize,
        max_replay_attempts: u8,
    ) -> Result<Self, crate::InvariantViolation> {
        if max_frame_bytes == 0 {
            return Err(crate::InvariantViolation::new(
                "snapshot frame limit must be non-zero",
            ));
        }
        Ok(Self {
            max_pending,
            max_queued,
            max_frame_bytes,
            max_replay_attempts,
        })
    }
}

/// Snapshot rejection raised before any live kernel is hydrated.
#[derive(Debug, Error)]
pub enum SnapshotError {
    /// The producer used a different snapshot schema.
    #[error("unsupported host snapshot format {found}")]
    UnsupportedVersion {
        /// Version found in the capsule.
        found: u16,
    },
    /// Serialized state exceeded an explicit restoration bound.
    #[error("host snapshot exceeds {resource} capacity")]
    Capacity {
        /// Bounded resource that was exhausted.
        resource: &'static str,
    },
    /// A serialized frame failed JSON-RPC validation.
    #[error("invalid frame in host snapshot: {0}")]
    InvalidFrame(#[from] FrameParseError),
    /// Cross-field snapshot state contradicted a kernel invariant.
    #[error("invalid host snapshot: {0}")]
    Invariant(&'static str),
}

/// Restored host-session kernel state ready to hydrate a live runtime.
#[derive(Debug, Clone)]
struct RestoredHostSessionKernel {
    /// Current public session phase.
    session_phase: SessionPhase,
    /// Captured initialize state, if any.
    initialization_seed: Option<InitializationSeed>,
    /// Live pending requests.
    pending: HashMap<RequestId, PendingRequest>,
    /// Backlog of client frames waiting on worker readiness.
    queued_frames: VecDeque<FramedMessage>,
    /// Next pending sequence number.
    next_pending_sequence: u64,
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
    fn from_restored(restored: RestoredHostSessionKernel) -> Self {
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
        let mut pending = self
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
        pending.sort_by_key(|request| request.sequence);
        let queued_frames = self
            .queued_frames
            .iter()
            .map(|frame| frame.payload().to_vec())
            .collect::<Vec<_>>();
        HostSessionKernelSnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
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

    /// Returns the exact worker replay seed for an established public session.
    pub fn replay_seed(&self) -> Result<Option<InitializationSeed>, HostRejection> {
        prepare_replay_seed(self.session_phase, self.initialization_seed.as_ref())
    }

    /// Observes a client frame before it is forwarded or queued.
    pub fn observe_client_frame(&mut self, frame: &FramedMessage) -> Result<(), HostRejection> {
        match frame.classify() {
            RpcEnvelopeKind::Request { id, method } if method.is_initialize() => {
                if self.session_phase != SessionPhase::Cold {
                    return Err(HostRejection::InvalidExecutionState);
                }
                self.initialization_seed = Some(InitializationSeed {
                    initialize_request: SeededInitializeRequest {
                        id,
                        payload: frame.payload().to_vec(),
                    },
                    initialized_notification: None,
                });
            }
            RpcEnvelopeKind::Notification { method } if method.is_initialized_notification() => {
                if self.session_phase != SessionPhase::AwaitingInitialized {
                    return Err(HostRejection::InvalidExecutionState);
                }
                let seed = self
                    .initialization_seed
                    .as_mut()
                    .ok_or(HostRejection::InvalidExecutionState)?;
                seed.initialized_notification = Some(frame.payload().to_vec());
                self.session_phase = SessionPhase::Live;
            }
            RpcEnvelopeKind::Request { .. }
            | RpcEnvelopeKind::Notification { .. }
            | RpcEnvelopeKind::Response { .. } => {}
        }
        Ok(())
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

    /// Returns the number of live pending invocations.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Returns whether no invocation is pending.
    #[must_use]
    pub fn pending_is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Returns immutable pending invocation state by public request ID.
    #[must_use]
    pub fn pending_request(&self, request_id: &RequestId) -> Option<&PendingRequest> {
        self.pending.get(request_id)
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
        let next_pending_sequence = sequence
            .checked_add(1)
            .ok_or(HostRejection::InvalidExecutionState)?;
        if method.is_initialize() {
            if self.session_phase != SessionPhase::Cold {
                return Err(HostRejection::InvalidExecutionState);
            }
            let seed_matches = self.initialization_seed.as_ref().is_some_and(|seed| {
                seed.initialize_request.id == id
                    && seed.initialize_request.payload.as_slice() == frame.payload()
            });
            if !seed_matches {
                self.initialization_seed = Some(InitializationSeed {
                    initialize_request: SeededInitializeRequest {
                        id: id.clone(),
                        payload: frame.payload().to_vec(),
                    },
                    initialized_notification: None,
                });
            }
            self.session_phase = SessionPhase::Initializing;
        }
        self.next_pending_sequence = next_pending_sequence;
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

    /// Records and removes one exact terminal JSON-RPC response.
    pub fn complete_response(
        &mut self,
        response: &FramedMessage,
    ) -> Result<CompletedPendingRequest, HostRejection> {
        let RpcEnvelopeKind::Response { id, has_error } = response.classify() else {
            return Err(HostRejection::InvalidRequestFrame);
        };
        let request_id = &id;
        let pending = self
            .pending
            .get(request_id)
            .ok_or(HostRejection::RequestNotPending)?;
        let completed_knowledge = pending
            .execution_knowledge
            .after_terminal_outcome()
            .map_err(|_| HostRejection::InvalidExecutionState)?;
        if pending.method.is_initialize() && self.session_phase != SessionPhase::Initializing {
            return Err(HostRejection::InvalidExecutionState);
        }
        let pending = self
            .pending
            .get_mut(request_id)
            .ok_or(HostRejection::InvalidExecutionState)?;
        pending.execution_knowledge = completed_knowledge;
        let request = self
            .pending
            .remove(request_id)
            .ok_or(HostRejection::InvalidExecutionState)?;
        if request.method.is_initialize() {
            if has_error {
                self.session_phase = SessionPhase::Cold;
                self.initialization_seed = None;
            } else {
                self.session_phase = SessionPhase::AwaitingInitialized;
            }
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
    /// Validates the complete capsule and restores one live kernel atomically.
    pub fn restore(self, limits: SnapshotLimits) -> Result<HostSessionKernel, SnapshotError> {
        if self.format_version != SNAPSHOT_FORMAT_VERSION {
            return Err(SnapshotError::UnsupportedVersion {
                found: self.format_version,
            });
        }
        if self.pending.len() > limits.max_pending {
            return Err(SnapshotError::Capacity {
                resource: "pending request",
            });
        }
        if self.queued_frames.len() > limits.max_queued {
            return Err(SnapshotError::Capacity {
                resource: "queued frame",
            });
        }

        let now = StdInstant::now();
        let mut pending = HashMap::with_capacity(self.pending.len());
        let mut sequences = HashSet::<u64>::with_capacity(self.pending.len());
        for snapshot in self.pending {
            if snapshot.frame.len() > limits.max_frame_bytes {
                return Err(SnapshotError::Capacity {
                    resource: "frame bytes",
                });
            }
            if snapshot.replay_attempts > limits.max_replay_attempts {
                return Err(SnapshotError::Capacity {
                    resource: "replay attempt",
                });
            }
            if !sequences.insert(snapshot.sequence)
                || snapshot.sequence >= self.next_pending_sequence
            {
                return Err(SnapshotError::Invariant(
                    "pending sequences must be unique and below the next sequence",
                ));
            }
            validate_scheduled_disposition(&snapshot)?;
            let frame = FramedMessage::parse(snapshot.frame)?;
            match frame.classify() {
                RpcEnvelopeKind::Request { id, method }
                    if id == snapshot.request_id && method == snapshot.method => {}
                RpcEnvelopeKind::Request { .. }
                | RpcEnvelopeKind::Notification { .. }
                | RpcEnvelopeKind::Response { .. } => {
                    return Err(SnapshotError::Invariant(
                        "pending identity diverges from its immutable frame",
                    ));
                }
            }
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
                return Err(SnapshotError::Invariant(
                    "duplicate request id in host snapshot",
                ));
            }
        }

        let mut queued_frames = VecDeque::with_capacity(self.queued_frames.len());
        let mut queued_ids = HashSet::<RequestId>::new();
        let mut scheduled_seen = HashSet::<RequestId>::new();
        let mut last_recovery_sequence = None::<u64>;
        let mut reached_client_backlog = false;
        for payload in self.queued_frames {
            if payload.len() > limits.max_frame_bytes {
                return Err(SnapshotError::Capacity {
                    resource: "frame bytes",
                });
            }
            let frame = FramedMessage::parse(payload)?;
            if let RpcEnvelopeKind::Request { id, .. } = frame.classify() {
                if !queued_ids.insert(id.clone()) {
                    return Err(SnapshotError::Invariant(
                        "duplicate request id in queued snapshot frames",
                    ));
                }
                if let Some(request) = pending.get(&id) {
                    if reached_client_backlog
                        || request.scheduled_disposition.is_none()
                        || request.frame.payload() != frame.payload()
                        || last_recovery_sequence
                            .is_some_and(|sequence| sequence >= request.sequence)
                    {
                        return Err(SnapshotError::Invariant(
                            "recovery queue identity or order is invalid",
                        ));
                    }
                    last_recovery_sequence = Some(request.sequence);
                    let _inserted = scheduled_seen.insert(id);
                } else {
                    reached_client_backlog = true;
                }
            } else {
                reached_client_backlog = true;
            }
            queued_frames.push_back(frame);
        }
        if pending.iter().any(|(id, request)| {
            request.scheduled_disposition.is_some() && !scheduled_seen.contains(id)
        }) {
            return Err(SnapshotError::Invariant(
                "scheduled recovery request is absent from the queue",
            ));
        }

        validate_session_snapshot(
            self.session_phase,
            self.initialization_seed.as_ref(),
            &pending,
            limits.max_frame_bytes,
        )?;

        Ok(HostSessionKernel::from_restored(
            RestoredHostSessionKernel {
                session_phase: self.session_phase,
                initialization_seed: self.initialization_seed,
                pending,
                queued_frames,
                next_pending_sequence: self.next_pending_sequence,
            },
        ))
    }
}

fn validate_scheduled_disposition(snapshot: &PendingRequestSnapshot) -> Result<(), SnapshotError> {
    let valid = match snapshot.scheduled_disposition {
        None => matches!(
            snapshot.execution_knowledge,
            ExecutionKnowledge::InFlight | ExecutionKnowledge::OutcomeUnknown
        ),
        Some(RequestDisposition::Replay) => {
            snapshot.execution_knowledge == ExecutionKnowledge::OutcomeUnknown
                && matches!(
                    snapshot.replay_contract,
                    ReplayContract::Convergent | ReplayContract::ProbeRequired
                )
        }
        Some(RequestDisposition::HoldForProbe) => {
            snapshot.execution_knowledge == ExecutionKnowledge::OutcomeUnknown
                && snapshot.replay_contract == ReplayContract::ProbeRequired
        }
        Some(
            RequestDisposition::FirstDispatch
            | RequestDisposition::AwaitTerminal
            | RequestDisposition::Completed
            | RequestDisposition::CompleteFromProbe
            | RequestDisposition::RejectAmbiguousOutcome
            | RequestDisposition::RejectReplayExhausted
            | RequestDisposition::RejectUnexpectedProbeResolution,
        ) => false,
    };
    if valid {
        Ok(())
    } else {
        Err(SnapshotError::Invariant(
            "pending execution state and scheduled disposition disagree",
        ))
    }
}

fn validate_session_snapshot(
    phase: SessionPhase,
    seed: Option<&InitializationSeed>,
    pending: &HashMap<RequestId, PendingRequest>,
    max_frame_bytes: usize,
) -> Result<(), SnapshotError> {
    let mut pending_initializes = pending
        .iter()
        .filter(|(_, request)| request.method.is_initialize());
    let pending_initialize = pending_initializes.next();
    if pending_initializes.next().is_some() {
        return Err(SnapshotError::Invariant(
            "snapshot contains multiple initialize invocations",
        ));
    }
    let Some(seed) = seed else {
        if phase == SessionPhase::Cold && pending_initialize.is_none() {
            return Ok(());
        }
        return Err(SnapshotError::Invariant(
            "session phase requires an initialization seed",
        ));
    };
    if seed.initialize_request.payload.len() > max_frame_bytes
        || seed
            .initialized_notification
            .as_ref()
            .is_some_and(|payload| payload.len() > max_frame_bytes)
    {
        return Err(SnapshotError::Capacity {
            resource: "initialization frame bytes",
        });
    }
    let initialize = FramedMessage::parse(seed.initialize_request.payload.clone())?;
    if !matches!(
        initialize.classify(),
        RpcEnvelopeKind::Request { id, method }
            if id == seed.initialize_request.id && method.is_initialize()
    ) {
        return Err(SnapshotError::Invariant(
            "initialization seed request identity is invalid",
        ));
    }
    if let Some(notification_payload) = &seed.initialized_notification {
        let notification = FramedMessage::parse(notification_payload.clone())?;
        if !matches!(
            notification.classify(),
            RpcEnvelopeKind::Notification { method } if method.is_initialized_notification()
        ) {
            return Err(SnapshotError::Invariant(
                "initialized seed frame is not the client notification",
            ));
        }
    }

    let phase_valid = match phase {
        SessionPhase::Cold => {
            pending_initialize.is_none() && seed.initialized_notification.is_none()
        }
        SessionPhase::Initializing => {
            seed.initialized_notification.is_none()
                && pending_initialize.is_some_and(|(id, request)| {
                    id == &seed.initialize_request.id
                        && request.frame.payload() == seed.initialize_request.payload.as_slice()
                })
        }
        SessionPhase::AwaitingInitialized => {
            pending_initialize.is_none() && seed.initialized_notification.is_none()
        }
        SessionPhase::Live => {
            pending_initialize.is_none() && seed.initialized_notification.is_some()
        }
    };
    if phase_valid {
        Ok(())
    } else {
        Err(SnapshotError::Invariant(
            "public session phase contradicts initialization state",
        ))
    }
}

/// Prepares a replay seed based on the current session phase.
pub fn prepare_replay_seed(
    session_phase: SessionPhase,
    initialization_seed: Option<&InitializationSeed>,
) -> Result<Option<InitializationSeed>, HostRejection> {
    match session_phase {
        SessionPhase::Cold | SessionPhase::Initializing => Ok(None),
        SessionPhase::AwaitingInitialized => initialization_seed
            .filter(|seed| seed.initialized_notification.is_none())
            .cloned()
            .map(Some)
            .ok_or(HostRejection::InvalidExecutionState),
        SessionPhase::Live => initialization_seed
            .filter(|seed| seed.initialized_notification.is_some())
            .cloned()
            .map(Some)
            .ok_or(HostRejection::InvalidExecutionState),
    }
}

/// Owned, private one-shot snapshot file prepared for process replacement.
#[derive(Debug)]
pub struct SnapshotCapsule {
    path: TempPath,
}

impl SnapshotCapsule {
    /// Returns the path to publish to the replacement process.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.path.as_ref()
    }
}

/// Serializes, flushes, and seals a private snapshot capsule for exec handoff.
pub fn write_snapshot_file<T>(prefix: &str, snapshot: &T) -> io::Result<SnapshotCapsule>
where
    T: Serialize,
{
    validate_snapshot_prefix(prefix)?;
    let serialized = serde_json::to_vec(snapshot).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to serialize host reexec snapshot: {error}"),
        )
    })?;
    let mut file = TempfileBuilder::new()
        .prefix(prefix)
        .suffix(".json")
        .tempfile_in(std::env::temp_dir())?;
    file.write_all(&serialized)?;
    file.flush()?;
    file.as_file().sync_all()?;
    Ok(SnapshotCapsule {
        path: file.into_temp_path(),
    })
}

/// Loads and deletes a snapshot file referenced by an environment variable.
pub fn load_snapshot_file_from_env<T>(env_var: &str, max_bytes: usize) -> io::Result<Option<T>>
where
    T: DeserializeOwned,
{
    let raw_path = std::env::var_os(env_var);
    let Some(raw_path) = raw_path.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    load_snapshot_file(Path::new(&raw_path), max_bytes).map(Some)
}

fn load_snapshot_file<T>(path: &Path, max_bytes: usize) -> io::Result<T>
where
    T: DeserializeOwned,
{
    if max_bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "snapshot byte limit must be non-zero",
        ));
    }
    let mut file = open_private_snapshot(path)?;
    let metadata = file.metadata()?;

    #[cfg(unix)]
    fs::remove_file(path)?;

    if metadata.len() > max_bytes as u64 {
        #[cfg(not(unix))]
        fs::remove_file(path)?;
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("host snapshot exceeds {max_bytes} byte limit"),
        ));
    }

    let read_limit = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut serialized = Vec::with_capacity(metadata.len() as usize);
    let mut bounded_file = io::Read::take(&mut file, read_limit);
    let _bytes_read = bounded_file.read_to_end(&mut serialized)?;

    #[cfg(not(unix))]
    fs::remove_file(path)?;

    if serialized.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("host snapshot exceeds {max_bytes} byte limit"),
        ));
    }
    serde_json::from_slice(&serialized).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to decode host reexec snapshot: {error}"),
        )
    })
}

fn validate_snapshot_prefix(prefix: &str) -> io::Result<()> {
    if prefix.is_empty()
        || prefix.len() > 64
        || !prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "snapshot prefix must be 1-64 portable filename characters",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn open_private_snapshot(path: &Path) -> io::Result<File> {
    use rustix::{
        fs::{Mode, OFlags, open},
        process::getuid,
    };
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let descriptor = open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?;
    let file = File::from(descriptor);
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != getuid().as_raw()
        || metadata.permissions().mode().trailing_zeros() < 6
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "snapshot capsule must be a private regular file owned by this user",
        ));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_private_snapshot(path: &Path) -> io::Result<File> {
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "snapshot capsule must be a regular file",
        ));
    }
    Ok(file)
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        DispatchQueueOutcome, FramedMessage, HostRejection, HostSessionKernel,
        HostSessionKernelSnapshot, InitializationSeed, PendingRequest, ProbeResolutionOutcome,
        ReplayBudget, RequestId, RpcMethod, SeededInitializeRequest, SessionPhase, SnapshotError,
        SnapshotLimits, load_snapshot_file, prepare_replay_seed, write_snapshot_file,
    };
    use crate::{ExecutionKnowledge, ProbeResolution, ReplayContract};
    use serde_json::json;

    #[test]
    fn replay_seed_preserves_the_exact_public_handshake() {
        let mut seed = InitializationSeed {
            initialize_request: SeededInitializeRequest {
                id: RequestId::number(1),
                payload: br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_vec(),
            },
            initialized_notification: None,
        };

        assert_eq!(
            prepare_replay_seed(SessionPhase::Cold, Some(&seed)),
            Ok(None)
        );
        assert!(matches!(
            prepare_replay_seed(SessionPhase::AwaitingInitialized, Some(&seed)),
            Ok(Some(prepared)) if prepared.initialized_notification.is_none()
        ));
        assert_eq!(
            prepare_replay_seed(SessionPhase::Live, Some(&seed)),
            Err(HostRejection::InvalidExecutionState)
        );

        seed.initialized_notification =
            Some(br#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#.to_vec());
        assert!(matches!(
            prepare_replay_seed(SessionPhase::Live, Some(&seed)),
            Ok(Some(prepared)) if prepared == seed
        ));
        assert_eq!(
            prepare_replay_seed(SessionPhase::AwaitingInitialized, Some(&seed)),
            Err(HostRejection::InvalidExecutionState)
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
            format_version: super::SNAPSHOT_FORMAT_VERSION,
            session_phase: SessionPhase::Live,
            initialization_seed: Some(InitializationSeed {
                initialize_request: SeededInitializeRequest {
                    id: RequestId::number(1),
                    payload: br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#
                        .to_vec(),
                },
                initialized_notification: Some(
                    br#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#
                        .to_vec(),
                ),
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

        let limits = match SnapshotLimits::try_new(8, 8, 1024 * 1024, 3) {
            Ok(limits) => limits,
            Err(_) => return,
        };
        let restored = snapshot.restore(limits);
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
    fn snapshot_restore_rejects_version_identity_bounds_and_phase_corruption() {
        let Some(request) = tool_request(71, "snapshot") else {
            return;
        };
        let mut kernel = HostSessionKernel::cold();
        assert!(
            kernel
                .begin_request_dispatch(&request, ReplayContract::Convergent, 4)
                .is_ok()
        );
        let snapshot = kernel.snapshot();
        let limits = match SnapshotLimits::try_new(4, 4, 1024, 2) {
            Ok(limits) => limits,
            Err(_) => return,
        };

        let mut wrong_version = snapshot.clone();
        wrong_version.format_version = 99;
        assert!(matches!(
            wrong_version.restore(limits),
            Err(SnapshotError::UnsupportedVersion { found: 99 })
        ));

        let mut divergent_identity = snapshot.clone();
        divergent_identity.pending[0].request_id = RequestId::number(72);
        assert!(matches!(
            divergent_identity.restore(limits),
            Err(SnapshotError::Invariant(_))
        ));

        let mut excessive_attempts = snapshot.clone();
        excessive_attempts.pending[0].replay_attempts = 3;
        assert!(matches!(
            excessive_attempts.restore(limits),
            Err(SnapshotError::Capacity {
                resource: "replay attempt"
            })
        ));

        let mut impossible_phase = snapshot;
        impossible_phase.session_phase = SessionPhase::Live;
        assert!(matches!(
            impossible_phase.restore(limits),
            Err(SnapshotError::Invariant(_))
        ));
    }

    #[test]
    fn snapshot_restore_validates_scheduled_recovery_order() {
        let Some(first) = tool_request(81, "first") else {
            return;
        };
        let Some(second) = tool_request(82, "second") else {
            return;
        };
        let mut kernel = HostSessionKernel::cold();
        assert!(
            kernel
                .begin_request_dispatch(&first, ReplayContract::Convergent, 4)
                .is_ok()
        );
        assert!(
            kernel
                .begin_request_dispatch(&second, ReplayContract::Convergent, 4)
                .is_ok()
        );
        let outcome = kernel.requeue_pending_for_replay(ReplayBudget {
            max_attempts: 2,
            queue_capacity: 4,
        });
        assert!(outcome.rejected.is_empty());
        let snapshot = kernel.snapshot();
        let limits = match SnapshotLimits::try_new(4, 4, 1024, 2) {
            Ok(limits) => limits,
            Err(_) => return,
        };
        assert!(snapshot.clone().restore(limits).is_ok());

        let mut reversed = snapshot;
        reversed.queued_frames.reverse();
        assert!(matches!(
            reversed.restore(limits),
            Err(SnapshotError::Invariant(_))
        ));
    }

    #[test]
    fn snapshot_capsules_are_private_bounded_and_one_shot() {
        let value = json!({"format": 1, "secret": "state"});
        let capsule = write_snapshot_file("libmcp-test-", &value);
        assert!(capsule.is_ok());
        let capsule = match capsule {
            Ok(capsule) => capsule,
            Err(_) => return,
        };
        let path = capsule.path().to_owned();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let metadata = std::fs::metadata(&path);
            assert!(
                matches!(metadata, Ok(metadata) if metadata.permissions().mode().trailing_zeros() >= 6)
            );
        }

        let restored = load_snapshot_file::<serde_json::Value>(&path, 1024);
        assert!(matches!(restored, Ok(restored) if restored == value));
        assert!(!path.exists());
        drop(capsule);

        let disposable = write_snapshot_file("libmcp-test-", &value);
        assert!(disposable.is_ok());
        let disposable = match disposable {
            Ok(capsule) => capsule,
            Err(_) => return,
        };
        let disposable_path = disposable.path().to_owned();
        drop(disposable);
        assert!(!disposable_path.exists());

        assert!(write_snapshot_file("../escape", &value).is_err());
    }

    #[test]
    fn rejected_snapshot_capsules_are_still_consumed() {
        let capsule = write_snapshot_file("libmcp-test-", &json!({"large": "payload"}));
        let capsule = match capsule {
            Ok(capsule) => capsule,
            Err(_) => return,
        };
        let path = capsule.path().to_owned();
        let rejected = load_snapshot_file::<serde_json::Value>(&path, 1);
        assert!(matches!(rejected, Err(error) if error.kind() == std::io::ErrorKind::InvalidData));
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_loader_refuses_symlink_handoffs() {
        use std::os::unix::fs::symlink;

        let capsule = write_snapshot_file("libmcp-test-", &json!({"state": true}));
        let capsule = match capsule {
            Ok(capsule) => capsule,
            Err(_) => return,
        };
        let directory = tempfile::tempdir();
        let directory = match directory {
            Ok(directory) => directory,
            Err(_) => return,
        };
        let link = directory.path().join("capsule.json");
        assert!(symlink(capsule.path(), &link).is_ok());
        assert!(load_snapshot_file::<serde_json::Value>(&link, 1024).is_err());
        assert!(link.exists());
        assert!(capsule.path().exists());
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

        let Some(first_response) = success_response(11) else {
            return;
        };
        assert!(kernel.complete_response(&first_response).is_ok());
        assert!(matches!(
            kernel.complete_response(&first_response),
            Err(HostRejection::RequestNotPending)
        ));
        assert!(
            kernel
                .begin_request_dispatch(&first, ReplayContract::Convergent, 1)
                .is_ok()
        );
    }

    #[test]
    fn public_initialization_advances_only_on_observed_protocol_events() {
        let initialize = FramedMessage::parse(
            br#"{"jsonrpc":"2.0","id":41,"method":"initialize","params":{}}"#.to_vec(),
        );
        let initialized = FramedMessage::parse(
            br#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#.to_vec(),
        );
        let Some(response) = success_response(41) else {
            return;
        };
        let (Ok(initialize), Ok(initialized)) = (initialize, initialized) else {
            return;
        };
        let mut kernel = HostSessionKernel::cold();

        assert!(kernel.observe_client_frame(&initialize).is_ok());
        assert_eq!(kernel.session_phase(), SessionPhase::Cold);
        assert!(
            kernel
                .begin_request_dispatch(&initialize, ReplayContract::Convergent, 1)
                .is_ok()
        );
        assert_eq!(kernel.session_phase(), SessionPhase::Initializing);
        assert_eq!(
            kernel.observe_client_frame(&initialized),
            Err(HostRejection::InvalidExecutionState)
        );
        assert!(kernel.complete_response(&response).is_ok());
        assert_eq!(kernel.session_phase(), SessionPhase::AwaitingInitialized);
        assert!(
            matches!(kernel.replay_seed(), Ok(Some(seed)) if seed.initialized_notification.is_none())
        );

        assert!(kernel.observe_client_frame(&initialized).is_ok());
        assert_eq!(kernel.session_phase(), SessionPhase::Live);
        assert!(
            matches!(kernel.replay_seed(), Ok(Some(seed)) if seed.initialized_notification.as_deref() == Some(initialized.payload()))
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
        let Some(response) = success_response(32) else {
            return;
        };
        assert!(kernel.complete_response(&response).is_ok());
        assert!(kernel.queue_is_empty());
        assert!(matches!(
            kernel.complete_response(&response),
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

    fn success_response(id: u64) -> Option<FramedMessage> {
        let payload = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {}
        }))
        .ok()?;
        FramedMessage::parse(payload).ok()
    }
}
