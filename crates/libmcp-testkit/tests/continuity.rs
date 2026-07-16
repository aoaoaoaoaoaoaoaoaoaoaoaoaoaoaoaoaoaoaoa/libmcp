//! Downstream-only conformance tests for the public continuity spine.

use libmcp::{
    DispatchQueueOutcome, ExecutionKnowledge, FrameParseError, FramedMessage, HostRejection,
    HostSessionKernel, HostSessionKernelSnapshot, JsonPorcelainConfig, ProbeResolution,
    ReplayAllowance, ReplayBudget, ReplayContract, RequestDisposition, RequestId, SessionPhase,
    SnapshotError, SnapshotLimits, request_disposition,
};
use libmcp_testkit::ChurnHarness;
use serde as _;
use serde_json::json;
use std::error::Error;

#[test]
fn downstream_recovery_obeys_contract_order_and_actual_attempt_accounting()
-> Result<(), Box<dyn Error>> {
    let convergent = request(1, "convergent")?;
    let probed = request(2, "probed")?;
    let forbidden = request(3, "forbidden")?;
    let mut harness = ChurnHarness::cold();
    let _id = harness.dispatch_first(&convergent, ReplayContract::Convergent, 8)?;
    let _id = harness.dispatch_first(&probed, ReplayContract::ProbeRequired, 8)?;
    let _id = harness.dispatch_first(&forbidden, ReplayContract::NeverReplay, 8)?;

    let recovery = harness.kill_worker(ReplayBudget {
        max_attempts: 1,
        queue_capacity: 8,
    });
    assert_eq!(recovery.held_for_probe, vec![RequestId::number(2)]);
    assert!(matches!(
        recovery.rejected.as_slice(),
        [rejected] if rejected.request_id == RequestId::number(3)
            && rejected.reason == HostRejection::AmbiguousOutcome
    ));
    assert!(matches!(
        harness.kernel().pending_request(&RequestId::number(1)),
        Some(request)
            if request.execution_knowledge() == ExecutionKnowledge::OutcomeUnknown
                && request.replay_attempts() == 0
    ));

    assert!(matches!(
        harness.dispatch_next()?,
        DispatchQueueOutcome::Replay(frame)
            if request_id(&frame).as_ref() == Some(&RequestId::number(1))
    ));
    assert!(matches!(
        harness.kernel().pending_request(&RequestId::number(1)),
        Some(request) if request.replay_attempts() == 1
    ));
    assert!(matches!(
        harness.dispatch_next()?,
        DispatchQueueOutcome::HeldForProbe { request_id }
            if request_id == RequestId::number(2)
    ));
    let _resolution =
        harness.resolve_probe(&RequestId::number(2), ProbeResolution::SafeToReplay, 1)?;
    assert!(matches!(
        harness.dispatch_next()?,
        DispatchQueueOutcome::Replay(frame)
            if request_id(&frame).as_ref() == Some(&RequestId::number(2))
    ));
    assert!(matches!(
        harness.dispatch_next()?,
        DispatchQueueOutcome::Empty
    ));
    assert!(
        harness
            .kernel()
            .pending_request(&RequestId::number(3))
            .is_none()
    );

    let ids = harness
        .dispatched()
        .iter()
        .filter_map(request_id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            RequestId::number(1),
            RequestId::number(2),
            RequestId::number(3),
            RequestId::number(1),
            RequestId::number(2),
        ]
    );
    Ok(())
}

#[test]
fn downstream_kernel_rejects_duplicate_and_double_terminal_ids() -> Result<(), Box<dyn Error>> {
    let request = request(11, "once")?;
    let response = response(11)?;
    let mut harness = ChurnHarness::cold();
    let _id = harness.dispatch_first(&request, ReplayContract::Convergent, 1)?;
    assert_eq!(
        harness.dispatch_first(&request, ReplayContract::Convergent, 1),
        Err(HostRejection::DuplicateRequestId)
    );
    let _completed = harness.complete(&response)?;
    assert!(matches!(
        harness.complete(&response),
        Err(HostRejection::RequestNotPending)
    ));
    let _reused = harness.dispatch_first(&request, ReplayContract::Convergent, 1)?;
    Ok(())
}

#[test]
fn downstream_worker_loss_respects_pre_and_post_dispatch_boundaries() -> Result<(), Box<dyn Error>>
{
    let queued = request(15, "queued")?;
    let completed = request(16, "completed")?;
    let completed_response = response(16)?;
    let mut kernel = HostSessionKernel::cold();
    kernel.queue_client_frame(queued.clone(), 4)?;
    let recovery = kernel.requeue_pending_for_replay(ReplayBudget {
        max_attempts: 1,
        queue_capacity: 4,
    });
    assert!(recovery.rejected.is_empty());
    let dequeued = kernel.pop_next_dispatch()?;
    assert!(matches!(
        dequeued,
        DispatchQueueOutcome::ClientFrame(ref frame) if frame.payload() == queued.payload()
    ));
    assert!(kernel.pending_is_empty());
    let _id = kernel.begin_request_dispatch(&queued, ReplayContract::NeverReplay, 4)?;

    let mut completed_kernel = HostSessionKernel::cold();
    let _id = completed_kernel.begin_request_dispatch(&completed, ReplayContract::Convergent, 4)?;
    let _completed = completed_kernel.complete_response(&completed_response)?;
    let recovery = completed_kernel.requeue_pending_for_replay(ReplayBudget {
        max_attempts: 1,
        queue_capacity: 4,
    });
    assert!(recovery.rejected.is_empty());
    assert!(completed_kernel.pending_is_empty());
    assert!(matches!(
        completed_kernel.pop_next_dispatch()?,
        DispatchQueueOutcome::Empty
    ));
    Ok(())
}

#[test]
fn downstream_recovery_capacity_rejects_without_overwrite() -> Result<(), Box<dyn Error>> {
    let first = request(17, "first")?;
    let second = request(18, "second")?;
    let mut kernel = HostSessionKernel::cold();
    let _id = kernel.begin_request_dispatch(&first, ReplayContract::Convergent, 2)?;
    let _id = kernel.begin_request_dispatch(&second, ReplayContract::Convergent, 2)?;
    let recovery = kernel.requeue_pending_for_replay(ReplayBudget {
        max_attempts: 1,
        queue_capacity: 1,
    });
    assert!(matches!(
        recovery.rejected.as_slice(),
        [rejected] if rejected.request_id == RequestId::number(18)
            && rejected.reason == HostRejection::QueueOverflow
    ));
    assert_eq!(kernel.pending_len(), 1);
    assert!(matches!(
        kernel.pop_next_dispatch()?,
        DispatchQueueOutcome::Replay(frame)
            if request_id(&frame).as_ref() == Some(&RequestId::number(17))
    ));
    Ok(())
}

#[test]
fn downstream_snapshot_roundtrip_preserves_recovery_and_rejects_versions()
-> Result<(), Box<dyn Error>> {
    let request = request(21, "snapshot")?;
    let mut kernel = HostSessionKernel::cold();
    let _id = kernel.begin_request_dispatch(&request, ReplayContract::Convergent, 4)?;
    let recovery = kernel.requeue_pending_for_replay(ReplayBudget {
        max_attempts: 2,
        queue_capacity: 4,
    });
    assert!(recovery.rejected.is_empty());
    let snapshot = kernel.snapshot();
    let serialized = serde_json::to_vec(&snapshot)?;
    let decoded = serde_json::from_slice::<HostSessionKernelSnapshot>(&serialized)?;
    let limits = SnapshotLimits::try_new(4, 4, 1024, 2)?;
    let mut restored = decoded.restore(limits)?;
    assert!(matches!(
        restored.pop_next_dispatch()?,
        DispatchQueueOutcome::Replay(frame)
            if request_id(&frame).as_ref() == Some(&RequestId::number(21))
    ));
    assert!(matches!(
        restored.pending_request(&RequestId::number(21)),
        Some(request) if request.replay_attempts() == 1
    ));

    let mut corrupt = serde_json::to_value(snapshot)?;
    corrupt["format_version"] = json!(999);
    let corrupt = serde_json::from_value::<HostSessionKernelSnapshot>(corrupt)?;
    assert!(matches!(
        corrupt.restore(limits),
        Err(SnapshotError::UnsupportedVersion { found: 999 })
    ));
    Ok(())
}

#[test]
fn downstream_public_initialization_never_invents_client_events() -> Result<(), Box<dyn Error>> {
    let initialize = FramedMessage::parse(
        br#"{"jsonrpc":"2.0","id":31,"method":"initialize","params":{}}"#.to_vec(),
    )?;
    let initialized = FramedMessage::parse(
        br#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#.to_vec(),
    )?;
    let response = response(31)?;
    let mut kernel = HostSessionKernel::cold();
    kernel.observe_client_frame(&initialize)?;
    let _id = kernel.begin_request_dispatch(&initialize, ReplayContract::Convergent, 1)?;
    assert_eq!(kernel.session_phase(), SessionPhase::Initializing);
    let _completed = kernel.complete_response(&response)?;
    assert_eq!(kernel.session_phase(), SessionPhase::AwaitingInitialized);
    assert!(
        matches!(kernel.replay_seed()?, Some(seed) if seed.initialized_notification().is_none())
    );
    kernel.observe_client_frame(&initialized)?;
    assert_eq!(kernel.session_phase(), SessionPhase::Live);
    assert!(
        matches!(kernel.replay_seed()?, Some(seed) if seed.initialized_notification().is_some())
    );
    Ok(())
}

#[test]
fn downstream_decision_and_render_laws_are_total_and_visible() -> Result<(), Box<dyn Error>> {
    let available = ReplayAllowance::new(0, 1);
    assert_eq!(
        request_disposition(
            ExecutionKnowledge::OutcomeUnknown,
            ReplayContract::Convergent,
            None,
            available,
        ),
        RequestDisposition::Replay
    );
    assert_eq!(
        request_disposition(
            ExecutionKnowledge::OutcomeUnknown,
            ReplayContract::ProbeRequired,
            None,
            available,
        ),
        RequestDisposition::HoldForProbe
    );
    assert_eq!(
        request_disposition(
            ExecutionKnowledge::OutcomeUnknown,
            ReplayContract::NeverReplay,
            None,
            available,
        ),
        RequestDisposition::RejectAmbiguousOutcome
    );

    let config = JsonPorcelainConfig::try_new(2, 8)?;
    let rendered = libmcp::render_json_porcelain(
        &json!({"alpha": "one", "beta": "two", "gamma": "three"}),
        config,
    );
    assert_eq!(rendered.lines().count(), 2);
    assert!(rendered.contains("omitted"));
    let scalar = libmcp::render_json_porcelain(&json!("quote\" newline\n tail"), config);
    assert!(scalar.contains('…'));
    assert!(serde_json::from_str::<String>(&scalar).is_ok());
    Ok(())
}

fn request(id: u64, name: &str) -> Result<FramedMessage, Box<dyn Error>> {
    let payload = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": name, "arguments": {}}
    }))?;
    Ok(FramedMessage::parse(payload)?)
}

fn response(id: u64) -> Result<FramedMessage, FrameParseError> {
    FramedMessage::parse(format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{}}}}"#).into_bytes())
}

fn request_id(frame: &FramedMessage) -> Option<RequestId> {
    match frame.classify() {
        libmcp::RpcEnvelopeKind::Request { id, .. } => Some(id),
        libmcp::RpcEnvelopeKind::Notification { .. } | libmcp::RpcEnvelopeKind::Response { .. } => {
            None
        }
    }
}
