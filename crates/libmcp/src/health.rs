//! Standard health and telemetry payloads.

use crate::{fault::Fault, jsonrpc::RpcMethod, types::Generation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Coarse lifecycle state of the live worker set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    /// No worker is currently alive.
    Cold,
    /// Startup is in progress.
    Starting,
    /// A worker is healthy and serving.
    Ready,
    /// Recovery is in progress after a fault.
    Recovering,
}

/// Rollout or reload state of the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RolloutState {
    /// No rollout is pending.
    Stable,
    /// A rollout has been detected but not yet executed.
    Pending,
    /// A rollout or reexec is in flight.
    Reloading,
}

/// Base health snapshot for a hardened MCP host or worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HealthSnapshot {
    /// Current lifecycle state.
    pub state: LifecycleState,
    /// Current generation.
    pub generation: Generation,
    /// Process uptime in milliseconds.
    pub uptime_ms: u64,
    /// Consecutive failures since the last healthy request.
    pub consecutive_failures: u32,
    /// Total restart count.
    pub restart_count: u64,
    /// Rollout state when the runtime exposes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollout: Option<RolloutState>,
    /// Most recent fault, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_fault: Option<Fault>,
}

/// Aggregate request totals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TelemetryTotals {
    /// Total requests observed.
    pub request_count: u64,
    /// Requests that completed successfully.
    pub success_count: u64,
    /// Requests that returned downstream response errors.
    pub response_error_count: u64,
    /// Requests that failed due to transport or process churn.
    pub transport_fault_count: u64,
    /// Requests retried by the runtime.
    pub retry_count: u64,
}

/// Per-method telemetry aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MethodTelemetry {
    /// Method name.
    pub method: RpcMethod,
    /// Total requests for this method.
    pub request_count: u64,
    /// Successful requests.
    pub success_count: u64,
    /// Response errors.
    pub response_error_count: u64,
    /// Transport/process faults.
    pub transport_fault_count: u64,
    /// Retry count.
    pub retry_count: u64,
    /// Most recent latency, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_latency_ms: Option<u64>,
    /// Maximum latency.
    pub max_latency_ms: u64,
    /// Average latency.
    pub avg_latency_ms: u64,
    /// Most recent error text, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Base telemetry snapshot for a hardened MCP runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TelemetrySnapshot {
    /// Process uptime in milliseconds.
    pub uptime_ms: u64,
    /// Current lifecycle state.
    pub state: LifecycleState,
    /// Current generation.
    pub generation: Generation,
    /// Consecutive failures since last clean success.
    pub consecutive_failures: u32,
    /// Total restart count.
    pub restart_count: u64,
    /// Aggregate totals.
    pub totals: TelemetryTotals,
    /// Per-method aggregates.
    pub methods: Vec<MethodTelemetry>,
    /// Most recent fault, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_fault: Option<Fault>,
}
