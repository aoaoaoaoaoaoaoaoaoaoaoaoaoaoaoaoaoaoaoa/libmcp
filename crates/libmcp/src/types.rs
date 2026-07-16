//! Fundamental library-wide types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;
use thiserror::Error;

/// A library invariant was violated.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("libmcp invariant violated: {detail}")]
pub struct InvariantViolation {
    detail: &'static str,
}

impl InvariantViolation {
    /// Creates a new invariant violation.
    #[must_use]
    pub const fn new(detail: &'static str) -> Self {
        Self { detail }
    }
}

/// Monotonic worker generation identifier.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct Generation(NonZeroU64);

impl Generation {
    /// Returns the first generation.
    #[must_use]
    pub const fn genesis() -> Self {
        Self(NonZeroU64::MIN)
    }

    /// Returns the inner integer value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    /// Constructs a generation from its non-zero wire value.
    pub fn try_new(generation: u64) -> Result<Self, InvariantViolation> {
        NonZeroU64::new(generation)
            .map(Self)
            .ok_or_else(|| InvariantViolation::new("worker generation must be non-zero"))
    }

    /// Advances to the next generation.
    ///
    /// Exhausting the identifier space is an invariant failure; it must not
    /// silently reuse the active generation.
    pub fn next(self) -> Result<Self, InvariantViolation> {
        self.get()
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .map(Self)
            .ok_or_else(|| InvariantViolation::new("worker generation exhausted u64"))
    }
}

impl TryFrom<u64> for Generation {
    type Error = InvariantViolation;

    fn try_from(generation: u64) -> Result<Self, Self::Error> {
        Self::try_new(generation)
    }
}

#[cfg(test)]
mod tests {
    use super::Generation;

    #[test]
    fn generation_rejects_zero_and_overflow() {
        assert!(Generation::try_new(0).is_err());
        assert_eq!(Generation::try_new(7).map(Generation::get), Ok(7));
        assert!(
            Generation::try_new(u64::MAX)
                .and_then(Generation::next)
                .is_err()
        );
    }

    #[test]
    fn generation_deserialization_preserves_non_zero_invariant() {
        assert!(serde_json::from_str::<Generation>("0").is_err());
        let generation = serde_json::from_str::<Generation>("9");
        assert!(matches!(generation, Ok(value) if value.get() == 9));
    }
}
