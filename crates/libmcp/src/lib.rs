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
pub mod telemetry;
pub mod types;

pub use fault::{Fault, FaultClass, FaultCode, RecoveryDirective};
pub use health::{
    HealthSnapshot, LifecycleState, MethodTelemetry, RolloutState, TelemetrySnapshot,
    TelemetryTotals,
};
pub use host::{
    CompletedPendingRequest, HostRejection, HostSessionKernel, HostSessionKernelSnapshot,
    InitializationSeed, PendingRequest, RejectedReplay, ReplayBudget, ReplayRequeueOutcome,
    RestoredHostSessionKernel, SeededInitializeRequest, SessionPhase, load_snapshot_file_from_env,
    prepare_replay_seed, remove_snapshot_file, snapshot_temp_path,
    synthesized_initialized_notification, write_snapshot_file,
};
pub use jsonrpc::{
    FrameReadOutcome, FramedMessage, RequestId, RpcEnvelopeKind, RpcMethod, ToolCallMeta, ToolName,
    parse_tool_call_meta, read_frame, write_frame,
};
pub use normalize::{
    NumericParseError, PathNormalizeError, normalize_ascii_token, normalize_local_path,
    parse_human_unsigned_u64, saturating_u64_to_usize,
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
pub use replay::ReplayContract;
pub use telemetry::{TelemetryLog, ToolErrorDetail, ToolOutcome};
pub use types::{Generation, InvariantViolation};

pub use libmcp_derive::{SelectorProjection, ToolProjection};
