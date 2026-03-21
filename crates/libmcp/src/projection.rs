//! Model-facing projection traits.

use crate::render::{DetailLevel, JsonPorcelainConfig, render_json_porcelain};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

const OVERVIEW_CONCISE_CONFIG: JsonPorcelainConfig = JsonPorcelainConfig {
    max_lines: 10,
    max_inline_chars: 144,
};
const OVERVIEW_FULL_CONFIG: JsonPorcelainConfig = JsonPorcelainConfig {
    max_lines: 28,
    max_inline_chars: 240,
};
const LIST_CONCISE_CONFIG: JsonPorcelainConfig = JsonPorcelainConfig {
    max_lines: 16,
    max_inline_chars: 128,
};
const LIST_FULL_CONFIG: JsonPorcelainConfig = JsonPorcelainConfig {
    max_lines: 32,
    max_inline_chars: 176,
};
const READ_CONCISE_CONFIG: JsonPorcelainConfig = JsonPorcelainConfig {
    max_lines: 18,
    max_inline_chars: 176,
};
const READ_FULL_CONFIG: JsonPorcelainConfig = JsonPorcelainConfig {
    max_lines: 40,
    max_inline_chars: 320,
};
const MUTATION_CONCISE_CONFIG: JsonPorcelainConfig = JsonPorcelainConfig {
    max_lines: 12,
    max_inline_chars: 160,
};
const MUTATION_FULL_CONFIG: JsonPorcelainConfig = JsonPorcelainConfig {
    max_lines: 24,
    max_inline_chars: 256,
};
const OPS_CONCISE_CONFIG: JsonPorcelainConfig = JsonPorcelainConfig {
    max_lines: 8,
    max_inline_chars: 160,
};
const OPS_FULL_CONFIG: JsonPorcelainConfig = JsonPorcelainConfig {
    max_lines: 24,
    max_inline_chars: 240,
};

/// Projection failure.
#[derive(Debug, Error)]
pub enum ProjectionError {
    /// Serialization failed while materializing the projection.
    #[error("failed to serialize projection: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Model-facing surface kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    /// One bounded snapshot answering “where are we now?”
    Overview,
    /// Enumeration surfaces such as lists and summaries.
    List,
    /// Focused one-object reads.
    Read,
    /// Mutation receipts.
    Mutation,
    /// Operational and health surfaces.
    Ops,
}

/// Projection policy derived from the surface kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionPolicy {
    /// Declared surface kind.
    pub kind: SurfaceKind,
    /// Whether opaque database identifiers are forbidden by doctrine.
    pub forbid_opaque_ids: bool,
    /// Whether the surface is reference-only and must not inline bodies.
    pub reference_only: bool,
    /// Concise porcelain bounds.
    pub concise_porcelain: JsonPorcelainConfig,
    /// Full porcelain bounds.
    pub full_porcelain: JsonPorcelainConfig,
}

impl ProjectionPolicy {
    /// Builds a policy from the type-level doctrine.
    #[must_use]
    pub fn from_surface(kind: SurfaceKind, forbid_opaque_ids: bool, reference_only: bool) -> Self {
        let (concise_porcelain, full_porcelain) = match kind {
            SurfaceKind::Overview => (OVERVIEW_CONCISE_CONFIG, OVERVIEW_FULL_CONFIG),
            SurfaceKind::List => (LIST_CONCISE_CONFIG, LIST_FULL_CONFIG),
            SurfaceKind::Read => (READ_CONCISE_CONFIG, READ_FULL_CONFIG),
            SurfaceKind::Mutation => (MUTATION_CONCISE_CONFIG, MUTATION_FULL_CONFIG),
            SurfaceKind::Ops => (OPS_CONCISE_CONFIG, OPS_FULL_CONFIG),
        };
        Self {
            kind,
            forbid_opaque_ids,
            reference_only,
            concise_porcelain,
            full_porcelain,
        }
    }
}

/// Slug-first selector projection for model-facing references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SelectorRef {
    /// Stable human-facing selector.
    pub slug: String,
    /// Optional human-facing title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Anything that can produce a selector reference.
pub trait SelectorProjection {
    /// Builds the selector reference.
    fn selector_ref(&self) -> SelectorRef;
}

/// Type-level surface doctrine.
pub trait SurfacePolicy {
    /// Declared surface kind.
    const KIND: SurfaceKind;
    /// Whether opaque database identifiers are forbidden by doctrine.
    const FORBID_OPAQUE_IDS: bool = true;
    /// Whether the surface is reference-only.
    const REFERENCE_ONLY: bool = false;

    /// Materializes the policy.
    #[must_use]
    fn projection_policy(&self) -> ProjectionPolicy {
        ProjectionPolicy::from_surface(Self::KIND, Self::FORBID_OPAQUE_IDS, Self::REFERENCE_ONLY)
    }
}

/// Structured concise/full projections.
pub trait StructuredProjection {
    /// Concise structured projection.
    fn concise_projection(&self) -> Result<Value, ProjectionError>;
    /// Full structured projection.
    fn full_projection(&self) -> Result<Value, ProjectionError>;
}

/// Happy-path trait for model-facing tool outputs.
pub trait ToolProjection: StructuredProjection + SurfacePolicy {
    /// Returns the structured projection for the chosen detail level.
    fn structured_projection(&self, detail: DetailLevel) -> Result<Value, ProjectionError> {
        match detail {
            DetailLevel::Concise => self.concise_projection(),
            DetailLevel::Full => self.full_projection(),
        }
    }

    /// Renders porcelain for the chosen detail level.
    fn porcelain_projection(&self, detail: DetailLevel) -> Result<String, ProjectionError> {
        let policy = self.projection_policy();
        let (value, config) = match detail {
            DetailLevel::Concise => (self.concise_projection()?, policy.concise_porcelain),
            DetailLevel::Full => (self.full_projection()?, policy.full_porcelain),
        };
        Ok(render_json_porcelain(&value, config))
    }
}

impl<T> ToolProjection for T where T: StructuredProjection + SurfacePolicy {}

/// Explicit escape hatch for already-curated JSON projections.
#[derive(Debug, Clone)]
pub struct FallbackJsonProjection {
    concise: Value,
    full: Value,
    kind: SurfaceKind,
    forbid_opaque_ids: bool,
    reference_only: bool,
}

impl FallbackJsonProjection {
    /// Builds a fallback projection from serialized values.
    pub fn new(
        concise: impl Serialize,
        full: impl Serialize,
        kind: SurfaceKind,
    ) -> Result<Self, ProjectionError> {
        Ok(Self {
            concise: serde_json::to_value(concise)?,
            full: serde_json::to_value(full)?,
            kind,
            forbid_opaque_ids: true,
            reference_only: false,
        })
    }

    /// Builds a fallback projection with explicit doctrine flags.
    pub fn with_policy(
        concise: impl Serialize,
        full: impl Serialize,
        kind: SurfaceKind,
        forbid_opaque_ids: bool,
        reference_only: bool,
    ) -> Result<Self, ProjectionError> {
        Ok(Self {
            concise: serde_json::to_value(concise)?,
            full: serde_json::to_value(full)?,
            kind,
            forbid_opaque_ids,
            reference_only,
        })
    }
}

impl StructuredProjection for FallbackJsonProjection {
    fn concise_projection(&self) -> Result<Value, ProjectionError> {
        Ok(self.concise.clone())
    }

    fn full_projection(&self) -> Result<Value, ProjectionError> {
        Ok(self.full.clone())
    }
}

impl SurfacePolicy for FallbackJsonProjection {
    const KIND: SurfaceKind = SurfaceKind::Read;

    fn projection_policy(&self) -> ProjectionPolicy {
        ProjectionPolicy::from_surface(self.kind, self.forbid_opaque_ids, self.reference_only)
    }
}

#[cfg(test)]
mod tests {
    use super::{StructuredProjection as _, SurfaceKind, SurfacePolicy as _};
    use crate::{DetailLevel, SelectorProjection, SelectorRef, ToolProjection};

    #[derive(Clone, SelectorProjection)]
    struct HypothesisSelector {
        slug: String,
        title: String,
    }

    #[derive(ToolProjection)]
    #[libmcp(kind = "read")]
    struct ExperimentProjection {
        slug: String,
        title: String,
        hypothesis: SelectorRef,
        #[libmcp(skip_none)]
        summary: Option<String>,
        #[libmcp(full_only)]
        analysis: String,
    }

    #[test]
    fn derived_projection_shapes_detail_levels() {
        let owner = HypothesisSelector {
            slug: "native-lp-sink".to_owned(),
            title: "Native LP sink".to_owned(),
        };
        let projection = ExperimentProjection {
            slug: "matched-lp-site-traces".to_owned(),
            title: "Matched LP site traces".to_owned(),
            hypothesis: owner.selector_ref(),
            summary: Some("Node LP work dominates traced native spend.".to_owned()),
            analysis: "Native LP spends most traced wallclock in node reoptimization.".to_owned(),
        };

        assert_eq!(ExperimentProjection::KIND, SurfaceKind::Read);

        let concise = projection.concise_projection();
        assert!(concise.is_ok());
        let concise = match concise {
            Ok(value) => value,
            Err(_) => return,
        };
        let full = projection.full_projection();
        assert!(full.is_ok());
        let full = match full {
            Ok(value) => value,
            Err(_) => return,
        };

        assert!(concise.get("analysis").is_none());
        assert_eq!(
            concise
                .get("hypothesis")
                .and_then(|value| value.get("slug"))
                .and_then(serde_json::Value::as_str),
            Some("native-lp-sink")
        );
        assert_eq!(
            full.get("analysis").and_then(serde_json::Value::as_str),
            Some("Native LP spends most traced wallclock in node reoptimization.")
        );

        let porcelain = projection.porcelain_projection(DetailLevel::Concise);
        assert!(porcelain.is_ok());
        let porcelain = match porcelain {
            Ok(value) => value,
            Err(_) => return,
        };
        assert!(porcelain.contains("slug: \"matched-lp-site-traces\""));
    }
}
