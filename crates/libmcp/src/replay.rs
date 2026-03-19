//! Replay contracts for request surfaces.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Replay legality for a request surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplayContract {
    /// Repeated execution converges on the same observable outcome.
    Convergent,
    /// Replay is only legal after a probe or equivalent proof of safety.
    ProbeRequired,
    /// Replay is never allowed automatically.
    NeverReplay,
}
