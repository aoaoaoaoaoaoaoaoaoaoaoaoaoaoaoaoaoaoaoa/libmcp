//! Durable public-session runtime primitives for hardened MCP hosts.

use crate::{
    jsonrpc::{
        FramedMessage, RequestId, RpcEnvelopeKind, RpcMethod, ToolCallMeta, parse_tool_call_meta,
    },
    replay::ReplayContract,
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
}

impl HostRejection {
    /// JSON-RPC error code for the rejection.
    #[must_use]
    pub const fn code(self) -> i64 {
        match self {
            Self::QueueOverflow => -32097,
            Self::ReplayBudgetExhausted => -32095,
        }
    }

    /// Human-facing rejection message.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::QueueOverflow => "worker queue overflow during recovery",
            Self::ReplayBudgetExhausted => "worker restart replay budget exhausted for request",
        }
    }
}

/// Live pending request tracked by the host.
#[derive(Debug, Clone)]
pub struct PendingRequest {
    /// Public JSON-RPC method.
    pub method: RpcMethod,
    /// Stable ordering sequence across retries.
    pub sequence: u64,
    /// Original request frame.
    pub frame: FramedMessage,
    /// Replay legality for the request surface.
    pub replay_contract: ReplayContract,
    /// Local start time for latency and age accounting.
    pub started_at: StdInstant,
    /// Best-effort tool metadata for telemetry grouping.
    pub tool_call_meta: Option<ToolCallMeta>,
}

/// Pending request plus the number of replay attempts consumed so far.
#[derive(Debug, Clone)]
pub struct CompletedPendingRequest {
    /// Pending request metadata and original frame.
    pub request: PendingRequest,
    /// Replay attempts consumed for this request.
    pub replay_attempts: u8,
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
    pub next_attempt: u8,
    /// Rejection reason.
    pub reason: HostRejection,
}

/// Result of rebuilding the replay queue after worker failure.
#[derive(Debug, Clone, Default)]
pub struct ReplayRequeueOutcome {
    /// Requests dropped during recovery.
    pub rejected: Vec<RejectedReplay>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingRequestSnapshot {
    request_id: RequestId,
    method: RpcMethod,
    sequence: u64,
    frame: Vec<u8>,
    replay_contract: ReplayContract,
    age_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReplayAttemptSnapshot {
    request_id: RequestId,
    attempts: u8,
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
    /// Retry counters for pending requests.
    replay_attempts: Vec<ReplayAttemptSnapshot>,
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
    /// Retry counters for pending requests.
    pub replay_attempts: HashMap<RequestId, u8>,
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
            replay_attempts: HashMap::new(),
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
    replay_attempts: HashMap<RequestId, u8>,
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
            replay_attempts: restored.replay_attempts,
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
                frame: request.frame.payload.clone(),
                replay_contract: request.replay_contract,
                age_ms: duration_millis_u64(request.started_at.elapsed()),
            })
            .collect::<Vec<_>>();
        let queued_frames = self
            .queued_frames
            .iter()
            .map(|frame| frame.payload.clone())
            .collect::<Vec<_>>();
        let replay_attempts = self
            .replay_attempts
            .iter()
            .map(|(request_id, attempts)| ReplayAttemptSnapshot {
                request_id: request_id.clone(),
                attempts: *attempts,
            })
            .collect::<Vec<_>>();
        HostSessionKernelSnapshot {
            session_phase: self.session_phase,
            initialization_seed,
            pending,
            replay_attempts,
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
                        payload: frame.payload.clone(),
                    },
                    initialized_notification: prior_initialized,
                });
            }
            RpcEnvelopeKind::Notification { method } if method.is_initialized_notification() => {
                if let Some(seed) = self.initialization_seed.as_mut() {
                    seed.initialized_notification = Some(frame.payload.clone());
                }
            }
            RpcEnvelopeKind::Request { .. }
            | RpcEnvelopeKind::Notification { .. }
            | RpcEnvelopeKind::Response { .. }
            | RpcEnvelopeKind::Unknown => {}
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
        self.queued_frames.push_back(frame);
        Ok(())
    }

    /// Pops the next queued client frame in order.
    pub fn pop_queued_frame(&mut self) -> Option<FramedMessage> {
        self.queued_frames.pop_front()
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

    /// Tracks a client request that has been forwarded to the worker.
    pub fn record_forwarded_request(
        &mut self,
        frame: &FramedMessage,
        replay_contract: ReplayContract,
    ) {
        if let RpcEnvelopeKind::Request { id, method } = frame.classify() {
            if method.is_initialize() {
                self.session_phase = SessionPhase::Live;
            }
            let parsed_tool_meta = parse_tool_call_meta(frame, &method);
            let prior = self.pending.get(&id).cloned();
            let (sequence, started_at, tool_call_meta) = if let Some(previous) = prior {
                (
                    previous.sequence,
                    previous.started_at,
                    previous.tool_call_meta.or(parsed_tool_meta),
                )
            } else {
                let sequence = self.next_pending_sequence;
                self.next_pending_sequence = self.next_pending_sequence.saturating_add(1);
                (sequence, StdInstant::now(), parsed_tool_meta)
            };
            let _previous = self.pending.insert(
                id,
                PendingRequest {
                    method,
                    sequence,
                    frame: frame.clone(),
                    replay_contract,
                    started_at,
                    tool_call_meta,
                },
            );
        }
    }

    /// Removes a pending request after a response arrives.
    pub fn take_completed_request(
        &mut self,
        request_id: &RequestId,
    ) -> Option<CompletedPendingRequest> {
        let request = self.pending.remove(request_id)?;
        let replay_attempts = self.replay_attempts.remove(request_id).unwrap_or(0);
        Some(CompletedPendingRequest {
            request,
            replay_attempts,
        })
    }

    /// Rebuilds the replay queue after worker failure.
    pub fn requeue_pending_for_replay(&mut self, budget: ReplayBudget) -> ReplayRequeueOutcome {
        let mut ordered_pending = self
            .pending
            .iter()
            .map(|(id, request)| (id.clone(), request.clone()))
            .collect::<Vec<_>>();
        ordered_pending.sort_by_key(|(_, request)| request.sequence);
        let pending_ids = self.pending.keys().cloned().collect::<HashSet<_>>();

        let mut replay_frames = VecDeque::<FramedMessage>::new();
        let mut dropped_ids = Vec::<RequestId>::new();
        let mut rejected = Vec::<RejectedReplay>::new();

        for (request_id, request) in ordered_pending {
            if request.method.is_initialize() {
                continue;
            }

            let attempts_used = self.replay_attempts.get(&request_id).copied().unwrap_or(0);
            let next_attempt = attempts_used.saturating_add(1);
            if next_attempt > budget.max_attempts {
                let _removed = self.replay_attempts.remove(&request_id);
                dropped_ids.push(request_id.clone());
                rejected.push(RejectedReplay {
                    request_id,
                    request,
                    next_attempt,
                    reason: HostRejection::ReplayBudgetExhausted,
                });
                continue;
            }

            if replay_frames.len().saturating_add(self.queued_frames.len()) >= budget.queue_capacity
            {
                let _removed = self.replay_attempts.remove(&request_id);
                dropped_ids.push(request_id.clone());
                rejected.push(RejectedReplay {
                    request_id,
                    request,
                    next_attempt,
                    reason: HostRejection::QueueOverflow,
                });
                continue;
            }

            let _previous = self.replay_attempts.insert(request_id, next_attempt);
            replay_frames.push_back(request.frame.clone());
        }

        for request_id in dropped_ids {
            let _removed = self.pending.remove(&request_id);
        }

        let mut retained_queue = VecDeque::<FramedMessage>::new();
        while let Some(frame) = self.queued_frames.pop_front() {
            let should_drop = match frame.classify() {
                RpcEnvelopeKind::Request { id, .. } => pending_ids.contains(&id),
                RpcEnvelopeKind::Notification { .. }
                | RpcEnvelopeKind::Response { .. }
                | RpcEnvelopeKind::Unknown => false,
            };
            if !should_drop {
                retained_queue.push_back(frame);
            }
        }

        replay_frames.append(&mut retained_queue);
        self.queued_frames = replay_frames;
        ReplayRequeueOutcome { rejected }
    }
}

impl HostSessionKernelSnapshot {
    /// Restores a live host-session state from the serialized snapshot.
    pub fn restore(self) -> io::Result<RestoredHostSessionKernel> {
        let now = StdInstant::now();
        let mut pending = HashMap::with_capacity(self.pending.len());
        for snapshot in self.pending {
            let frame = FramedMessage::parse(snapshot.frame)?;
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
            queued_frames.push_back(FramedMessage::parse(payload)?);
        }

        let replay_attempts = self
            .replay_attempts
            .into_iter()
            .map(|snapshot| (snapshot.request_id, snapshot.attempts))
            .collect::<HashMap<_, _>>();

        Ok(RestoredHostSessionKernel {
            session_phase: self.session_phase,
            initialization_seed: self.initialization_seed,
            pending,
            replay_attempts,
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
        FramedMessage, HostRejection, HostSessionKernel, HostSessionKernelSnapshot,
        InitializationSeed, ReplayBudget, RequestId, RpcMethod, SeededInitializeRequest,
        SessionPhase, prepare_replay_seed, synthesized_initialized_notification,
    };
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
                replay_contract: crate::ReplayContract::Convergent,
                age_ms: 25,
            }],
            replay_attempts: vec![super::ReplayAttemptSnapshot {
                request_id: RequestId::number(7),
                attempts: 2,
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
        assert_eq!(
            restored.replay_attempts.get(&RequestId::number(7)).copied(),
            Some(2)
        );
        let pending = restored.pending.get(&RequestId::number(7));
        assert!(pending.is_some(), "expected pending request to round-trip");
        let pending = match pending {
            Some(value) => value,
            None => return,
        };
        assert_eq!(pending.sequence, 3);
        assert!(pending.method.is_tools_call());
        assert_eq!(pending.replay_contract, crate::ReplayContract::Convergent);
        assert!(
            pending.tool_call_meta.is_some(),
            "expected tool metadata to be reconstructed from replay snapshot"
        );
    }

    #[test]
    fn replay_requeue_drops_requests_that_exhaust_budget() {
        let initialize = FramedMessage::parse(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_vec(),
        );
        assert!(initialize.is_ok());
        let initialize = match initialize {
            Ok(value) => value,
            Err(_) => return,
        };
        let diagnostics = FramedMessage::parse(
            br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"diagnostics","arguments":{"file_path":"/tmp/lib.rs"}}}"#.to_vec(),
        );
        assert!(diagnostics.is_ok());
        let diagnostics = match diagnostics {
            Ok(value) => value,
            Err(_) => return,
        };

        let mut kernel = HostSessionKernel::cold();
        kernel.observe_client_frame(&initialize);
        kernel.record_forwarded_request(&initialize, crate::ReplayContract::Convergent);
        kernel.record_forwarded_request(&diagnostics, crate::ReplayContract::Convergent);
        let _previous = kernel.replay_attempts.insert(RequestId::number(2), 1);

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
}
