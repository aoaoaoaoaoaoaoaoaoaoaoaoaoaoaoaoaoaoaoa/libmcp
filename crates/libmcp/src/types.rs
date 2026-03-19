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

    /// Advances to the next generation, saturating on overflow.
    #[must_use]
    pub fn next(self) -> Self {
        let next = self.get().saturating_add(1);
        let non_zero = NonZeroU64::new(next).map_or(NonZeroU64::MAX, |value| value);
        Self(non_zero)
    }
}
