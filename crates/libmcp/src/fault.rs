//! Fault taxonomy and recovery directives.

use crate::types::Generation;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de};

/// Broad operational fault class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FaultClass {
    /// Underlying transport or I/O failure.
    Transport,
    /// Process startup, liveness, or exit failure.
    Process,
    /// Protocol or framing failure.
    Protocol,
    /// Timeout or deadline failure.
    Timeout,
    /// Downstream service returned an error.
    Downstream,
    /// Resource budget or queue exhaustion.
    Resource,
    /// Replay or recovery budget exhaustion.
    Replay,
    /// Rollout or binary handoff failure.
    Rollout,
    /// Internal invariant breach.
    Invariant,
}

/// Recovery directive for an operational fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryDirective {
    /// Retry on the current live process.
    RetryInPlace,
    /// Restart or roll forward, then replay if the replay contract allows it.
    RestartAndReplay,
    /// Abort the request and surface the failure.
    AbortRequest,
}

/// A typed but extensible fault code.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct FaultCode(String);

impl FaultCode {
    /// Constructs a new fault code.
    ///
    /// The code must be non-empty and use lowercase ASCII with underscores.
    pub fn try_new(code: impl Into<String>) -> Result<Self, crate::types::InvariantViolation> {
        let code = code.into();
        if code.is_empty()
            || !code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_' || byte.is_ascii_digit())
        {
            return Err(crate::types::InvariantViolation::new(
                "fault code must be non-empty lowercase ascii snake_case",
            ));
        }
        Ok(Self(code))
    }

    /// Returns the code text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl<'de> Deserialize<'de> for FaultCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let code = String::deserialize(deserializer)?;
        Self::try_new(code).map_err(de::Error::custom)
    }
}

/// Structured operational fault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Fault {
    /// Generation in which the fault happened.
    pub generation: Generation,
    /// Broad fault class.
    pub class: FaultClass,
    /// Consumer-defined fine-grained code.
    pub code: FaultCode,
    /// Recovery directive implied by this fault.
    pub directive: RecoveryDirective,
    /// Human-facing detail.
    pub detail: String,
}

impl Fault {
    /// Constructs a new fault.
    #[must_use]
    pub fn new(
        generation: Generation,
        class: FaultClass,
        code: FaultCode,
        directive: RecoveryDirective,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            generation,
            class,
            code,
            directive,
            detail: detail.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FaultCode;

    #[test]
    fn fault_code_rejects_non_snake_case() {
        assert!(FaultCode::try_new("broken_pipe").is_ok());
        assert!(FaultCode::try_new("BrokenPipe").is_err());
        assert!(FaultCode::try_new("").is_err());
        assert!(serde_json::from_str::<FaultCode>(r#""BrokenPipe""#).is_err());
        assert!(serde_json::from_str::<FaultCode>(r#""broken_pipe""#).is_ok());
    }
}
