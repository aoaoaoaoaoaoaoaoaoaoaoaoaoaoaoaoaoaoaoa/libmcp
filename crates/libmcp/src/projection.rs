//! Model-facing projection traits.

use crate::render::{DetailLevel, JsonPorcelainConfig, render_json_porcelain};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;
use std::fmt;
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const OVERVIEW_BOUNDS: SurfacePorcelainBounds = SurfacePorcelainBounds {
    concise: JsonPorcelainConfig::fixed(10, 144),
    full: JsonPorcelainConfig::fixed(28, 240),
};
const LIST_BOUNDS: SurfacePorcelainBounds = SurfacePorcelainBounds {
    concise: JsonPorcelainConfig::fixed(16, 128),
    full: JsonPorcelainConfig::fixed(32, 176),
};
const READ_BOUNDS: SurfacePorcelainBounds = SurfacePorcelainBounds {
    concise: JsonPorcelainConfig::fixed(18, 176),
    full: JsonPorcelainConfig::fixed(40, 320),
};
const MUTATION_BOUNDS: SurfacePorcelainBounds = SurfacePorcelainBounds {
    concise: JsonPorcelainConfig::fixed(12, 160),
    full: JsonPorcelainConfig::fixed(24, 256),
};
const OPS_BOUNDS: SurfacePorcelainBounds = SurfacePorcelainBounds {
    concise: JsonPorcelainConfig::fixed(8, 160),
    full: JsonPorcelainConfig::fixed(24, 240),
};

/// Projection failure.
#[derive(Debug, Error)]
pub enum ProjectionError {
    /// Serialization failed while materializing the projection.
    #[error("failed to serialize projection: {0}")]
    Serialize(#[from] serde_json::Error),
    /// A declared surface policy was violated by the materialized projection.
    #[error("projection policy violation at {path}: {rule}")]
    Policy {
        /// JSON-like location of the violation.
        path: String,
        /// Stable policy rule description.
        rule: &'static str,
    },
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

/// Detail-indexed porcelain bounds for one surface kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfacePorcelainBounds {
    /// Concise porcelain bounds.
    pub concise: JsonPorcelainConfig,
    /// Full porcelain bounds.
    pub full: JsonPorcelainConfig,
}

impl SurfacePorcelainBounds {
    /// Selects bounds for a detail level.
    #[must_use]
    pub const fn for_detail(self, detail: DetailLevel) -> JsonPorcelainConfig {
        match detail {
            DetailLevel::Concise => self.concise,
            DetailLevel::Full => self.full,
        }
    }
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
    /// Detail-indexed porcelain bounds.
    pub porcelain: SurfacePorcelainBounds,
}

impl ProjectionPolicy {
    /// Builds a policy from the type-level doctrine.
    #[must_use]
    pub fn from_surface(kind: SurfaceKind, forbid_opaque_ids: bool, reference_only: bool) -> Self {
        let porcelain = match kind {
            SurfaceKind::Overview => OVERVIEW_BOUNDS,
            SurfaceKind::List => LIST_BOUNDS,
            SurfaceKind::Read => READ_BOUNDS,
            SurfaceKind::Mutation => MUTATION_BOUNDS,
            SurfaceKind::Ops => OPS_BOUNDS,
        };
        Self {
            kind,
            forbid_opaque_ids,
            reference_only,
            porcelain,
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

/// Uniform RFC3339 timestamp text for model-facing surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct TimestampText(String);

impl TimestampText {
    /// Parses and canonicalizes RFC3339 timestamp text.
    pub fn try_new(timestamp: impl AsRef<str>) -> Result<Self, crate::InvariantViolation> {
        let timestamp = OffsetDateTime::parse(timestamp.as_ref(), &Rfc3339)
            .map_err(|_| crate::InvariantViolation::new("timestamp text must be valid RFC3339"))?;
        Self::try_from(timestamp)
    }

    /// Returns the rendered timestamp string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TimestampText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<OffsetDateTime> for TimestampText {
    type Error = crate::InvariantViolation;

    fn try_from(timestamp: OffsetDateTime) -> Result<Self, Self::Error> {
        timestamp.format(&Rfc3339).map(Self).map_err(|_| {
            crate::InvariantViolation::new("timestamp is outside the RFC3339 year range")
        })
    }
}

impl<'de> Deserialize<'de> for TimestampText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let timestamp = String::deserialize(deserializer)?;
        Self::try_new(timestamp).map_err(de::Error::custom)
    }
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
        let value = match detail {
            DetailLevel::Concise => self.concise_projection(),
            DetailLevel::Full => self.full_projection(),
        }?;
        validate_projection_policy(&value, self.projection_policy())?;
        Ok(value)
    }

    /// Renders porcelain for the chosen detail level.
    fn porcelain_projection(&self, detail: DetailLevel) -> Result<String, ProjectionError> {
        let policy = self.projection_policy();
        let value = self.structured_projection(detail)?;
        let config = policy.porcelain.for_detail(detail);
        Ok(render_json_porcelain(&value, config))
    }
}

impl<T> ToolProjection for T where T: StructuredProjection + SurfacePolicy {}

fn validate_projection_policy(
    value: &Value,
    policy: ProjectionPolicy,
) -> Result<(), ProjectionError> {
    walk_projection(value, "$", &mut |path, object| {
        for key in object.keys() {
            if policy.forbid_opaque_ids && (key == "id" || key.ends_with("_id")) {
                return Err(ProjectionError::Policy {
                    path: format!("{path}.{key}"),
                    rule: "opaque identifier fields are forbidden",
                });
            }
            if policy.reference_only
                && matches!(key.as_str(), "body" | "content" | "text" | "bytes")
            {
                return Err(ProjectionError::Policy {
                    path: format!("{path}.{key}"),
                    rule: "reference-only surfaces must not inline artifact content",
                });
            }
            if policy.kind == SurfaceKind::List
                && matches!(
                    key.as_str(),
                    "body" | "payload_preview" | "analysis" | "rationale"
                )
            {
                return Err(ProjectionError::Policy {
                    path: format!("{path}.{key}"),
                    rule: "list surfaces must not inline prose bodies",
                });
            }
        }
        Ok(())
    })
}

fn walk_projection(
    value: &Value,
    path: &str,
    visitor: &mut impl FnMut(&str, &serde_json::Map<String, Value>) -> Result<(), ProjectionError>,
) -> Result<(), ProjectionError> {
    match value {
        Value::Object(object) => {
            visitor(path, object)?;
            for (key, child) in object {
                walk_projection(child, &format!("{path}.{key}"), visitor)?;
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                walk_projection(child, &format!("{path}[{index}]"), visitor)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

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
    use super::{
        FallbackJsonProjection, StructuredProjection as _, SurfaceKind, SurfacePolicy as _,
    };
    use crate::{DetailLevel, SelectorProjection, SelectorRef, ToolProjection};
    use time::OffsetDateTime;

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

    #[test]
    fn timestamp_text_serializes_as_rfc3339_string() {
        let timestamp = OffsetDateTime::from_unix_timestamp(0);
        assert!(timestamp.is_ok());
        let timestamp = match timestamp {
            Ok(value) => value,
            Err(_) => return,
        };
        let rendered = super::TimestampText::try_from(timestamp);
        assert!(rendered.is_ok());
        let rendered = match rendered {
            Ok(value) => value,
            Err(_) => return,
        };
        let json = serde_json::to_value(&rendered);
        assert!(json.is_ok());
        let json = match json {
            Ok(value) => value,
            Err(_) => return,
        };
        assert_eq!(json, serde_json::json!("1970-01-01T00:00:00Z"));
        assert!(serde_json::from_str::<super::TimestampText>(r#""not-a-time""#).is_err());
    }

    #[test]
    fn tool_projection_enforces_declared_policy_before_returning_output() {
        let projection = FallbackJsonProjection::with_policy(
            serde_json::json!({"id": 7, "slug": "visible"}),
            serde_json::json!({"id": 7, "slug": "visible"}),
            SurfaceKind::Read,
            true,
            false,
        );
        assert!(projection.is_ok());
        let projection = match projection {
            Ok(projection) => projection,
            Err(_) => return,
        };
        assert!(
            projection
                .structured_projection(DetailLevel::Concise)
                .is_err()
        );
        assert!(projection.porcelain_projection(DetailLevel::Full).is_err());
    }
}
