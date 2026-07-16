//! Standard health and telemetry payloads.

use crate::{fault::Fault, jsonrpc::RpcMethod, types::Generation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

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

/// Handshake state of the active disposable worker generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkerHandshakePhase {
    /// No worker is attached.
    Absent,
    /// A process is starting but has not begun MCP initialization.
    Starting,
    /// The active worker is performing its private initialization handshake.
    Initializing,
    /// The active worker completed its private handshake.
    Ready,
    /// The active generation failed and awaits replacement.
    Failed,
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
    /// Active worker handshake state, distinct from host lifecycle.
    pub worker_handshake: WorkerHandshakePhase,
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
    request_count: u64,
    /// Requests that completed successfully.
    success_count: u64,
    /// Requests that completed with any public error response.
    error_count: u64,
    /// Requests that completed with downstream response errors.
    response_error_count: u64,
    /// Requests that completed with host-recovery errors.
    recovery_error_count: u64,
    /// Recovery-triggering operational faults observed.
    recovery_fault_count: u64,
    /// Requests retried by the runtime.
    retry_count: u64,
}

impl TelemetryTotals {
    /// Returns total admitted public requests.
    #[must_use]
    pub const fn request_count(&self) -> u64 {
        self.request_count
    }

    /// Returns terminal public successes.
    #[must_use]
    pub const fn success_count(&self) -> u64 {
        self.success_count
    }

    /// Returns all terminal public errors.
    #[must_use]
    pub const fn error_count(&self) -> u64 {
        self.error_count
    }

    /// Returns terminal downstream response errors.
    #[must_use]
    pub const fn response_error_count(&self) -> u64 {
        self.response_error_count
    }

    /// Returns terminal public errors caused by host recovery.
    #[must_use]
    pub const fn recovery_error_count(&self) -> u64 {
        self.recovery_error_count
    }

    /// Returns recovery-triggering operational fault incidents.
    #[must_use]
    pub const fn recovery_fault_count(&self) -> u64 {
        self.recovery_fault_count
    }

    /// Returns actual redispatches.
    #[must_use]
    pub const fn retry_count(&self) -> u64 {
        self.retry_count
    }
}

/// Per-method telemetry aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MethodTelemetry {
    /// Method name.
    method: RpcMethod,
    /// Total requests for this method.
    request_count: u64,
    /// Successful requests.
    success_count: u64,
    /// All terminal public errors.
    error_count: u64,
    /// Downstream response errors.
    response_error_count: u64,
    /// Terminal errors caused by host recovery.
    recovery_error_count: u64,
    /// Recovery-triggering operational fault incidents.
    recovery_fault_count: u64,
    /// Retry count.
    retry_count: u64,
    /// Most recent latency, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_latency_ms: Option<u64>,
    /// Maximum latency.
    max_latency_ms: u64,
    /// Average latency.
    avg_latency_ms: u64,
    /// Most recent error text, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

impl MethodTelemetry {
    /// Returns the canonical method identity.
    #[must_use]
    pub const fn method(&self) -> &RpcMethod {
        &self.method
    }

    /// Returns admitted public requests for this method.
    #[must_use]
    pub const fn request_count(&self) -> u64 {
        self.request_count
    }

    /// Returns terminal public successes for this method.
    #[must_use]
    pub const fn success_count(&self) -> u64 {
        self.success_count
    }

    /// Returns all terminal public errors for this method.
    #[must_use]
    pub const fn error_count(&self) -> u64 {
        self.error_count
    }

    /// Returns terminal downstream response errors for this method.
    #[must_use]
    pub const fn response_error_count(&self) -> u64 {
        self.response_error_count
    }

    /// Returns terminal host-recovery errors for this method.
    #[must_use]
    pub const fn recovery_error_count(&self) -> u64 {
        self.recovery_error_count
    }

    /// Returns recovery-triggering fault incidents for this method.
    #[must_use]
    pub const fn recovery_fault_count(&self) -> u64 {
        self.recovery_fault_count
    }

    /// Returns actual redispatches for this method.
    #[must_use]
    pub const fn retry_count(&self) -> u64 {
        self.retry_count
    }

    /// Returns the most recent measured terminal latency.
    #[must_use]
    pub const fn last_latency_ms(&self) -> Option<u64> {
        self.last_latency_ms
    }

    /// Returns the maximum measured terminal latency.
    #[must_use]
    pub const fn max_latency_ms(&self) -> u64 {
        self.max_latency_ms
    }

    /// Returns the mean measured terminal latency.
    #[must_use]
    pub const fn avg_latency_ms(&self) -> u64 {
        self.avg_latency_ms
    }

    /// Returns the most recent public or operational error detail.
    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }
}

/// Base telemetry snapshot for a hardened MCP runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TelemetrySnapshot {
    /// Process uptime in milliseconds.
    pub uptime_ms: u64,
    /// Current lifecycle state.
    pub state: LifecycleState,
    /// Active worker handshake state.
    pub worker_handshake: WorkerHandshakePhase,
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

/// Session-scoped operational counters that survive worker churn and reexec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OperationalLedger {
    generation: Generation,
    consecutive_failures: u32,
    restart_count: u64,
    totals: TelemetryTotals,
    methods: BTreeMap<RpcMethod, MethodLedger>,
    last_fault: Option<Fault>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
struct MethodLedger {
    request_count: u64,
    in_flight: u64,
    success_count: u64,
    error_count: u64,
    response_error_count: u64,
    recovery_error_count: u64,
    recovery_fault_count: u64,
    retry_count: u64,
    latency_sample_count: u64,
    total_latency_ms: u64,
    last_latency_ms: Option<u64>,
    max_latency_ms: u64,
    last_error: Option<String>,
}

/// Rejected operational counter transition.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OperationalMetricError {
    /// A fixed-width counter was exhausted.
    #[error("operational metric counter exhausted: {0}")]
    CounterExhausted(&'static str),
    /// A terminal outcome or replay named no in-flight invocation.
    #[error("method has no in-flight invocation")]
    NoInFlightInvocation,
    /// Worker generations must strictly advance on replacement.
    #[error("replacement worker generation must strictly increase")]
    GenerationDidNotAdvance,
    /// A fault was attributed to a different active generation.
    #[error("fault generation does not match the active worker generation")]
    FaultGenerationMismatch,
}

impl OperationalLedger {
    /// Creates empty public-session metrics for the active worker generation.
    #[must_use]
    pub fn new(generation: Generation) -> Self {
        Self {
            generation,
            consecutive_failures: 0,
            restart_count: 0,
            totals: TelemetryTotals {
                request_count: 0,
                success_count: 0,
                error_count: 0,
                response_error_count: 0,
                recovery_error_count: 0,
                recovery_fault_count: 0,
                retry_count: 0,
            },
            methods: BTreeMap::new(),
            last_fault: None,
        }
    }

    /// Records admission of one public request.
    pub fn record_request(&mut self, method: RpcMethod) -> Result<(), OperationalMetricError> {
        let mut next = self.clone();
        let method_metrics = next.methods.entry(method).or_default();
        method_metrics.request_count =
            checked_increment(method_metrics.request_count, "method requests")?;
        method_metrics.in_flight = checked_increment(method_metrics.in_flight, "method in flight")?;
        next.totals.request_count = checked_increment(next.totals.request_count, "total requests")?;
        *self = next;
        Ok(())
    }

    /// Records one terminal successful public response.
    pub fn record_success(
        &mut self,
        method: &RpcMethod,
        latency_ms: u64,
    ) -> Result<(), OperationalMetricError> {
        let mut next = self.clone();
        let metrics = next.live_method_mut(method)?;
        record_latency(metrics, latency_ms)?;
        metrics.success_count = checked_increment(metrics.success_count, "method successes")?;
        metrics.in_flight -= 1;
        next.totals.success_count =
            checked_increment(next.totals.success_count, "total successes")?;
        next.consecutive_failures = 0;
        *self = next;
        Ok(())
    }

    /// Records one terminal downstream response error.
    pub fn record_response_error(
        &mut self,
        method: &RpcMethod,
        latency_ms: u64,
        detail: impl Into<String>,
    ) -> Result<(), OperationalMetricError> {
        let detail = detail.into();
        let mut next = self.clone();
        let metrics = next.live_method_mut(method)?;
        record_latency(metrics, latency_ms)?;
        metrics.response_error_count =
            checked_increment(metrics.response_error_count, "method response errors")?;
        metrics.error_count = checked_increment(metrics.error_count, "method errors")?;
        metrics.in_flight -= 1;
        metrics.last_error = Some(detail);
        next.totals.response_error_count =
            checked_increment(next.totals.response_error_count, "total response errors")?;
        next.totals.error_count = checked_increment(next.totals.error_count, "total errors")?;
        *self = next;
        Ok(())
    }

    /// Records a terminal public error caused by host recovery.
    pub fn record_recovery_error(
        &mut self,
        method: &RpcMethod,
        detail: impl Into<String>,
    ) -> Result<(), OperationalMetricError> {
        let detail = detail.into();
        let mut next = self.clone();
        let metrics = next.live_method_mut(method)?;
        metrics.recovery_error_count =
            checked_increment(metrics.recovery_error_count, "method recovery errors")?;
        metrics.error_count = checked_increment(metrics.error_count, "method errors")?;
        metrics.in_flight -= 1;
        metrics.last_error = Some(detail);
        next.totals.recovery_error_count =
            checked_increment(next.totals.recovery_error_count, "total recovery errors")?;
        next.totals.error_count = checked_increment(next.totals.error_count, "total errors")?;
        *self = next;
        Ok(())
    }

    /// Records a recovery-triggering fault without pretending it was terminal.
    pub fn record_recovery_fault(
        &mut self,
        method: Option<&RpcMethod>,
        fault: Fault,
    ) -> Result<(), OperationalMetricError> {
        if fault.generation != self.generation {
            return Err(OperationalMetricError::FaultGenerationMismatch);
        }
        let mut next = self.clone();
        if let Some(method) = method {
            let metrics = next.live_method_mut(method)?;
            metrics.recovery_fault_count =
                checked_increment(metrics.recovery_fault_count, "method recovery faults")?;
            metrics.last_error = Some(fault.detail.clone());
        }
        next.totals.recovery_fault_count =
            checked_increment(next.totals.recovery_fault_count, "total recovery faults")?;
        next.consecutive_failures = next.consecutive_failures.checked_add(1).ok_or(
            OperationalMetricError::CounterExhausted("consecutive failures"),
        )?;
        next.last_fault = Some(fault);
        *self = next;
        Ok(())
    }

    /// Records one actual request redispatch.
    pub fn record_replay(&mut self, method: &RpcMethod) -> Result<(), OperationalMetricError> {
        let mut next = self.clone();
        let metrics = next.live_method_mut(method)?;
        metrics.retry_count = checked_increment(metrics.retry_count, "method retries")?;
        next.totals.retry_count = checked_increment(next.totals.retry_count, "total retries")?;
        *self = next;
        Ok(())
    }

    /// Advances to a replacement worker and preserves public-session totals.
    pub fn replace_worker(&mut self, generation: Generation) -> Result<(), OperationalMetricError> {
        if generation <= self.generation {
            return Err(OperationalMetricError::GenerationDidNotAdvance);
        }
        let mut next = self.clone();
        next.restart_count = checked_increment(next.restart_count, "worker restarts")?;
        next.generation = generation;
        *self = next;
        Ok(())
    }

    /// Materializes the shared health view for the current host process.
    #[must_use]
    pub fn health_snapshot(
        &self,
        uptime_ms: u64,
        state: LifecycleState,
        worker_handshake: WorkerHandshakePhase,
        rollout: Option<RolloutState>,
    ) -> HealthSnapshot {
        HealthSnapshot {
            state,
            worker_handshake,
            generation: self.generation,
            uptime_ms,
            consecutive_failures: self.consecutive_failures,
            restart_count: self.restart_count,
            rollout,
            last_fault: self.last_fault.clone(),
        }
    }

    /// Materializes deterministic per-method session telemetry.
    #[must_use]
    pub fn telemetry_snapshot(
        &self,
        uptime_ms: u64,
        state: LifecycleState,
        worker_handshake: WorkerHandshakePhase,
    ) -> TelemetrySnapshot {
        let methods = self
            .methods
            .iter()
            .map(|(method, metrics)| method_snapshot(method, metrics))
            .collect();
        TelemetrySnapshot {
            uptime_ms,
            state,
            worker_handshake,
            generation: self.generation,
            consecutive_failures: self.consecutive_failures,
            restart_count: self.restart_count,
            totals: self.totals.clone(),
            methods,
            last_fault: self.last_fault.clone(),
        }
    }

    fn live_method_mut(
        &mut self,
        method: &RpcMethod,
    ) -> Result<&mut MethodLedger, OperationalMetricError> {
        self.methods
            .get_mut(method)
            .filter(|metrics| metrics.in_flight > 0)
            .ok_or(OperationalMetricError::NoInFlightInvocation)
    }
}

fn checked_increment(value: u64, name: &'static str) -> Result<u64, OperationalMetricError> {
    value
        .checked_add(1)
        .ok_or(OperationalMetricError::CounterExhausted(name))
}

fn record_latency(
    metrics: &mut MethodLedger,
    latency_ms: u64,
) -> Result<(), OperationalMetricError> {
    metrics.latency_sample_count =
        checked_increment(metrics.latency_sample_count, "method latency samples")?;
    metrics.total_latency_ms = metrics.total_latency_ms.checked_add(latency_ms).ok_or(
        OperationalMetricError::CounterExhausted("method total latency"),
    )?;
    metrics.last_latency_ms = Some(latency_ms);
    metrics.max_latency_ms = metrics.max_latency_ms.max(latency_ms);
    Ok(())
}

fn method_snapshot(method: &RpcMethod, metrics: &MethodLedger) -> MethodTelemetry {
    let avg_latency_ms = metrics
        .total_latency_ms
        .checked_div(metrics.latency_sample_count)
        .unwrap_or(0);
    MethodTelemetry {
        method: method.clone(),
        request_count: metrics.request_count,
        success_count: metrics.success_count,
        error_count: metrics.error_count,
        response_error_count: metrics.response_error_count,
        recovery_error_count: metrics.recovery_error_count,
        recovery_fault_count: metrics.recovery_fault_count,
        retry_count: metrics.retry_count,
        last_latency_ms: metrics.last_latency_ms,
        max_latency_ms: metrics.max_latency_ms,
        avg_latency_ms,
        last_error: metrics.last_error.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{LifecycleState, OperationalLedger, RolloutState, WorkerHandshakePhase};
    use crate::{Fault, FaultClass, FaultCode, Generation, RecoveryHint, RpcMethod};

    #[test]
    fn ledger_preserves_session_scope_and_deterministic_method_order() {
        let mut ledger = OperationalLedger::new(Generation::genesis());
        let zeta = RpcMethod::try_new("zeta");
        let alpha = RpcMethod::try_new("alpha");
        let (Ok(zeta), Ok(alpha)) = (zeta, alpha) else {
            return;
        };
        assert!(ledger.record_request(zeta.clone()).is_ok());
        assert!(ledger.record_request(alpha.clone()).is_ok());
        assert!(ledger.record_replay(&zeta).is_ok());
        assert!(ledger.record_success(&zeta, 12).is_ok());
        assert!(
            ledger
                .record_response_error(&alpha, 8, "downstream")
                .is_ok()
        );

        let replacement = Generation::try_new(2);
        let Ok(replacement) = replacement else {
            return;
        };
        assert!(ledger.replace_worker(replacement).is_ok());
        let snapshot =
            ledger.telemetry_snapshot(4, LifecycleState::Ready, WorkerHandshakePhase::Ready);
        assert_eq!(snapshot.generation, replacement);
        assert_eq!(snapshot.restart_count, 1);
        assert_eq!(snapshot.totals.request_count(), 2);
        assert_eq!(snapshot.totals.success_count(), 1);
        assert_eq!(snapshot.totals.error_count(), 1);
        assert_eq!(snapshot.totals.response_error_count(), 1);
        assert_eq!(snapshot.totals.recovery_error_count(), 0);
        assert_eq!(snapshot.totals.retry_count(), 1);
        assert!(matches!(
            snapshot.methods.as_slice(),
            [first, second] if first.method().as_str() == "alpha" && second.method().as_str() == "zeta"
        ));
    }

    #[test]
    fn ledger_separates_host_lifecycle_worker_handshake_and_fault_scope() {
        let mut ledger = OperationalLedger::new(Generation::genesis());
        let method = RpcMethod::try_new("tools/call");
        let code = FaultCode::try_new("worker_lost");
        let (Ok(method), Ok(code)) = (method, code) else {
            return;
        };
        assert!(ledger.record_request(method.clone()).is_ok());
        let fault = Fault::new(
            Generation::genesis(),
            FaultClass::Transport,
            code,
            Some(RecoveryHint::ReplaceWorker),
            "worker pipe closed",
        );
        assert!(ledger.record_recovery_fault(Some(&method), fault).is_ok());
        let health = ledger.health_snapshot(
            9,
            LifecycleState::Recovering,
            WorkerHandshakePhase::Starting,
            Some(RolloutState::Stable),
        );
        assert_eq!(health.state, LifecycleState::Recovering);
        assert_eq!(health.worker_handshake, WorkerHandshakePhase::Starting);
        assert_eq!(health.consecutive_failures, 1);
        let telemetry = ledger.telemetry_snapshot(
            9,
            LifecycleState::Recovering,
            WorkerHandshakePhase::Starting,
        );
        assert_eq!(telemetry.totals.recovery_fault_count(), 1);
        assert_eq!(telemetry.totals.error_count(), 0);
        assert!(ledger.record_success(&method, 20).is_ok());
        let healthy =
            ledger.health_snapshot(10, LifecycleState::Ready, WorkerHandshakePhase::Ready, None);
        assert_eq!(healthy.consecutive_failures, 0);
    }

    #[test]
    fn ledger_distinguishes_terminal_recovery_errors_from_fault_incidents() {
        let mut ledger = OperationalLedger::new(Generation::genesis());
        let method = RpcMethod::try_new("tools/call");
        let Ok(method) = method else {
            return;
        };
        assert!(ledger.record_request(method.clone()).is_ok());
        assert!(
            ledger
                .record_recovery_error(&method, "ambiguous outcome")
                .is_ok()
        );

        let snapshot =
            ledger.telemetry_snapshot(5, LifecycleState::Ready, WorkerHandshakePhase::Ready);
        assert_eq!(snapshot.totals.request_count(), 1);
        assert_eq!(snapshot.totals.success_count(), 0);
        assert_eq!(snapshot.totals.error_count(), 1);
        assert_eq!(snapshot.totals.response_error_count(), 0);
        assert_eq!(snapshot.totals.recovery_error_count(), 1);
        assert_eq!(snapshot.totals.recovery_fault_count(), 0);
        assert!(matches!(
            snapshot.methods.as_slice(),
            [method] if method.error_count() == 1
                && method.recovery_error_count() == 1
                && method.recovery_fault_count() == 0
        ));
    }
}
