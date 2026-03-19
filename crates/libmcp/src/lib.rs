//! `libmcp` is the shared operational spine for hardened MCP servers.

pub mod fault;
pub mod health;
pub mod jsonrpc;
pub mod normalize;
pub mod render;
pub mod replay;
pub mod telemetry;
pub mod types;

pub use fault::{Fault, FaultClass, FaultCode, RecoveryDirective};
pub use health::{
    HealthSnapshot, LifecycleState, MethodTelemetry, RolloutState, TelemetrySnapshot,
    TelemetryTotals,
};
pub use jsonrpc::{
    FrameReadOutcome, FramedMessage, RequestId, RpcEnvelopeKind, ToolCallMeta,
    parse_tool_call_meta, read_frame, write_frame,
};
pub use normalize::{
    NumericParseError, PathNormalizeError, normalize_ascii_token, normalize_local_path,
    parse_human_unsigned_u64, saturating_u64_to_usize,
};
pub use render::{PathStyle, RenderConfig, RenderMode, TruncatedText, collapse_inline_whitespace};
pub use replay::ReplayContract;
pub use telemetry::{TelemetryLog, ToolErrorDetail, ToolOutcome};
pub use types::{Generation, InvariantViolation};
