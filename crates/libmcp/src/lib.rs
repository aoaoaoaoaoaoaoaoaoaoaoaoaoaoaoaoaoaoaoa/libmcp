//! `libmcp` is the shared operational spine for hardened MCP servers.

extern crate self as libmcp;

/// Implementation details used by `libmcp` derive expansions.
#[doc(hidden)]
pub mod __macro {
    pub use serde;
    pub use serde_json;
}

pub mod fault;
pub mod health;
pub mod host;
pub mod jsonrpc;
pub mod normalize;
pub mod projection;
pub mod render;
pub mod replay;
pub mod rollout;
pub mod telemetry;
pub mod types;

pub use fault::{Fault, FaultClass, FaultCode, RecoveryHint};
pub use health::{
    HealthSnapshot, LifecycleState, MethodTelemetry, OperationalLedger, OperationalMetricError,
    RolloutState, TelemetrySnapshot, TelemetryTotals, WorkerHandshakePhase,
};
pub use host::{
    CompletedPendingRequest, DispatchQueueOutcome, HostRejection, HostSessionKernel,
    HostSessionKernelSnapshot, InitializationSeed, PendingRequest, ProbeResolutionOutcome,
    RejectedReplay, ReplayBudget, ReplayRequeueOutcome, SNAPSHOT_FORMAT_VERSION,
    SeededInitializeRequest, SessionPhase, SnapshotCapsule, SnapshotError, SnapshotLimits,
    load_snapshot_file_from_env, write_snapshot_file,
};
pub use jsonrpc::{
    FrameLimit, FrameParseError, FrameReadOutcome, FramedMessage, RequestId, RpcEnvelopeKind,
    RpcMethod, ToolCallMeta, ToolName, parse_tool_call_meta, read_frame, read_frame_blocking,
    write_frame, write_frame_blocking,
};
#[cfg(unix)]
pub use jsonrpc::{TimedFrameReadOutcome, TimedFrameReader};
pub use normalize::{
    NumericParseError, PathNormalizeError, checked_u64_to_usize, normalize_local_path,
    parse_human_unsigned_u64,
};
pub use projection::{
    FallbackJsonProjection, ProjectionError, ProjectionPolicy, SelectorProjection, SelectorRef,
    StructuredProjection, SurfaceKind, SurfacePolicy, SurfacePorcelainBounds, TimestampText,
    ToolProjection,
};
pub use render::{
    DetailLevel, JsonPorcelainConfig, PathStyle, RenderConfig, RenderMode, TruncatedText,
    collapse_inline_whitespace, render_json_porcelain, with_presentation_properties,
};
pub use replay::{
    ExecutionKnowledge, ExecutionTransitionError, ProbeResolution, ReplayAllowance, ReplayContract,
    RequestDisposition, request_disposition,
};
pub use rollout::{
    HandoffOutcome, LIBMCP_HANDOFF_SOCKET_ENV, LIBMCP_RELEASE_CHANNEL_ENV,
    LIBMCP_RELEASE_GENERATION_ENV, ReleaseId, ReleaseManifest, ReleaseObservation, ReleasePointer,
    ReleaseProvenance, ReleaseRuntime, StateCompatibility, load_release, verify_release,
};
pub use telemetry::{TelemetryFlushPolicy, TelemetryLog, ToolErrorDetail, ToolOutcome};
pub use types::{Generation, InvariantViolation};

pub use libmcp_derive::{SelectorProjection, ToolProjection};
