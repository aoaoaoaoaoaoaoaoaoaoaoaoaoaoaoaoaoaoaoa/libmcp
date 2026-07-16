//! Replay contracts for request surfaces.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// What the host knows about one invocation's execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionKnowledge {
    /// The invocation definitely has not reached a worker.
    NotDispatched,
    /// The invocation reached a worker and no terminal outcome has arrived.
    InFlight,
    /// One terminal outcome has arrived.
    Completed,
    /// The worker was lost after dispatch, so effects may have happened.
    OutcomeUnknown,
}

impl ExecutionKnowledge {
    /// Applies loss of the active worker without inventing execution evidence.
    #[must_use]
    pub const fn after_worker_loss(self) -> Self {
        match self {
            Self::InFlight => Self::OutcomeUnknown,
            Self::NotDispatched | Self::Completed | Self::OutcomeUnknown => self,
        }
    }

    /// Applies a dispatch authorized by the recovery decision table.
    pub fn after_dispatch(
        self,
        disposition: RequestDisposition,
    ) -> Result<Self, ExecutionTransitionError> {
        match (self, disposition) {
            (Self::NotDispatched, RequestDisposition::FirstDispatch)
            | (Self::OutcomeUnknown, RequestDisposition::Replay) => Ok(Self::InFlight),
            _ => Err(ExecutionTransitionError {
                knowledge: self,
                disposition,
            }),
        }
    }

    /// Records one observed terminal worker outcome.
    pub fn after_terminal_outcome(self) -> Result<Self, ExecutionTransitionError> {
        if matches!(self, Self::InFlight | Self::OutcomeUnknown) {
            Ok(Self::Completed)
        } else {
            Err(ExecutionTransitionError {
                knowledge: self,
                disposition: RequestDisposition::Completed,
            })
        }
    }

    /// Applies a consumer probe that proves the prior attempt completed.
    pub fn after_completed_probe(self) -> Result<Self, ExecutionTransitionError> {
        if self == Self::OutcomeUnknown {
            Ok(Self::Completed)
        } else {
            Err(ExecutionTransitionError {
                knowledge: self,
                disposition: RequestDisposition::CompleteFromProbe,
            })
        }
    }
}

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

/// Consumer evidence resolving a `ProbeRequired` ambiguous outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProbeResolution {
    /// The prior attempt is known to have completed and must not run again.
    AlreadyCompleted,
    /// Domain evidence proves another dispatch is safe.
    SafeToReplay,
}

/// Replay attempt accounting for one invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReplayAllowance {
    attempts_used: u8,
    max_attempts: u8,
}

impl ReplayAllowance {
    /// Constructs replay accounting from attempts actually dispatched.
    #[must_use]
    pub const fn new(attempts_used: u8, max_attempts: u8) -> Self {
        Self {
            attempts_used,
            max_attempts,
        }
    }

    /// Returns attempts actually dispatched so far.
    #[must_use]
    pub const fn attempts_used(self) -> u8 {
        self.attempts_used
    }

    /// Returns the configured attempt ceiling.
    #[must_use]
    pub const fn max_attempts(self) -> u8 {
        self.max_attempts
    }

    /// Returns the next attempt number without consuming it.
    #[must_use]
    pub const fn next_attempt(self) -> Option<u8> {
        match self.attempts_used.checked_add(1) {
            Some(next) if next <= self.max_attempts => Some(next),
            Some(_) | None => None,
        }
    }
}

/// Total request disposition produced by the recovery law.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequestDisposition {
    /// Dispatch a request that definitely never reached a worker.
    FirstDispatch,
    /// Await the terminal result of the active attempt.
    AwaitTerminal,
    /// Preserve an already observed terminal result.
    Completed,
    /// Redispatch an invocation whose prior outcome is unknown.
    Replay,
    /// Keep the invocation held until the consumer supplies probe evidence.
    HoldForProbe,
    /// Complete without redispatch because a probe proved prior completion.
    CompleteFromProbe,
    /// Reject because execution may have happened and replay is forbidden.
    RejectAmbiguousOutcome,
    /// Reject because no replay attempt remains.
    RejectReplayExhausted,
    /// Reject contradictory or irrelevant probe evidence.
    RejectUnexpectedProbeResolution,
}

/// Invalid application of a request disposition to execution knowledge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("cannot apply {disposition:?} to invocation in {knowledge:?}")]
pub struct ExecutionTransitionError {
    knowledge: ExecutionKnowledge,
    disposition: RequestDisposition,
}

/// Computes the request disposition independently of process recovery policy.
#[must_use]
pub const fn request_disposition(
    knowledge: ExecutionKnowledge,
    contract: ReplayContract,
    probe_resolution: Option<ProbeResolution>,
    allowance: ReplayAllowance,
) -> RequestDisposition {
    if probe_resolution.is_some()
        && !(matches!(knowledge, ExecutionKnowledge::OutcomeUnknown)
            && matches!(contract, ReplayContract::ProbeRequired))
    {
        return RequestDisposition::RejectUnexpectedProbeResolution;
    }

    match knowledge {
        ExecutionKnowledge::NotDispatched => RequestDisposition::FirstDispatch,
        ExecutionKnowledge::InFlight => RequestDisposition::AwaitTerminal,
        ExecutionKnowledge::Completed => RequestDisposition::Completed,
        ExecutionKnowledge::OutcomeUnknown => match contract {
            ReplayContract::Convergent => replay_or_exhausted(allowance),
            ReplayContract::ProbeRequired => match probe_resolution {
                None => RequestDisposition::HoldForProbe,
                Some(ProbeResolution::AlreadyCompleted) => RequestDisposition::CompleteFromProbe,
                Some(ProbeResolution::SafeToReplay) => replay_or_exhausted(allowance),
            },
            ReplayContract::NeverReplay => RequestDisposition::RejectAmbiguousOutcome,
        },
    }
}

const fn replay_or_exhausted(allowance: ReplayAllowance) -> RequestDisposition {
    if allowance.next_attempt().is_some() {
        RequestDisposition::Replay
    } else {
        RequestDisposition::RejectReplayExhausted
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ExecutionKnowledge, ProbeResolution, ReplayAllowance, ReplayContract, RequestDisposition,
        request_disposition,
    };

    const AVAILABLE: ReplayAllowance = ReplayAllowance::new(0, 1);
    const EXHAUSTED: ReplayAllowance = ReplayAllowance::new(1, 1);

    #[test]
    fn steady_execution_states_ignore_replay_contract() {
        for contract in [
            ReplayContract::Convergent,
            ReplayContract::ProbeRequired,
            ReplayContract::NeverReplay,
        ] {
            assert_eq!(
                request_disposition(ExecutionKnowledge::NotDispatched, contract, None, EXHAUSTED,),
                RequestDisposition::FirstDispatch
            );
            assert_eq!(
                request_disposition(ExecutionKnowledge::InFlight, contract, None, AVAILABLE,),
                RequestDisposition::AwaitTerminal
            );
            assert_eq!(
                request_disposition(ExecutionKnowledge::Completed, contract, None, AVAILABLE,),
                RequestDisposition::Completed
            );
        }
    }

    #[test]
    fn ambiguous_outcomes_obey_contract_evidence_and_budget() {
        let unknown = ExecutionKnowledge::OutcomeUnknown;
        assert_eq!(
            request_disposition(unknown, ReplayContract::Convergent, None, AVAILABLE),
            RequestDisposition::Replay
        );
        assert_eq!(
            request_disposition(unknown, ReplayContract::Convergent, None, EXHAUSTED),
            RequestDisposition::RejectReplayExhausted
        );
        assert_eq!(
            request_disposition(unknown, ReplayContract::ProbeRequired, None, AVAILABLE),
            RequestDisposition::HoldForProbe
        );
        assert_eq!(
            request_disposition(
                unknown,
                ReplayContract::ProbeRequired,
                Some(ProbeResolution::AlreadyCompleted),
                AVAILABLE,
            ),
            RequestDisposition::CompleteFromProbe
        );
        assert_eq!(
            request_disposition(
                unknown,
                ReplayContract::ProbeRequired,
                Some(ProbeResolution::SafeToReplay),
                AVAILABLE,
            ),
            RequestDisposition::Replay
        );
        assert_eq!(
            request_disposition(unknown, ReplayContract::NeverReplay, None, AVAILABLE),
            RequestDisposition::RejectAmbiguousOutcome
        );
    }

    #[test]
    fn probe_evidence_cannot_authorize_an_unrelated_invocation() {
        assert_eq!(
            request_disposition(
                ExecutionKnowledge::OutcomeUnknown,
                ReplayContract::NeverReplay,
                Some(ProbeResolution::SafeToReplay),
                AVAILABLE,
            ),
            RequestDisposition::RejectUnexpectedProbeResolution
        );
        assert_eq!(
            request_disposition(
                ExecutionKnowledge::NotDispatched,
                ReplayContract::ProbeRequired,
                Some(ProbeResolution::SafeToReplay),
                AVAILABLE,
            ),
            RequestDisposition::RejectUnexpectedProbeResolution
        );
    }

    #[test]
    fn state_transitions_reject_unauthorized_replay_and_double_completion() {
        assert_eq!(
            ExecutionKnowledge::NotDispatched.after_dispatch(RequestDisposition::FirstDispatch),
            Ok(ExecutionKnowledge::InFlight)
        );
        assert!(
            ExecutionKnowledge::OutcomeUnknown
                .after_dispatch(RequestDisposition::FirstDispatch)
                .is_err()
        );
        assert_eq!(
            ExecutionKnowledge::InFlight.after_worker_loss(),
            ExecutionKnowledge::OutcomeUnknown
        );
        assert_eq!(
            ExecutionKnowledge::OutcomeUnknown.after_terminal_outcome(),
            Ok(ExecutionKnowledge::Completed)
        );
        assert!(
            ExecutionKnowledge::Completed
                .after_terminal_outcome()
                .is_err()
        );
    }
}
