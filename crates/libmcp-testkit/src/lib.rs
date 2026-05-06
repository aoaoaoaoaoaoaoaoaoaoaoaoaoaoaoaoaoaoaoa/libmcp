//! Shared test helpers for `libmcp` consumers.

use libmcp::{SurfaceKind, ToolProjection};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{
    fs::File,
    io::{self, BufRead, BufReader},
    path::Path,
};

const OPAQUE_ID_FIELD: &str = "id";
const OPAQUE_ID_SUFFIX: &str = "_id";
const LIST_BODY_FIELDS: &[&str] = &["body", "payload_preview", "analysis", "rationale"];
const REFERENCE_BODY_FIELDS: &[&str] = &["body", "content", "text", "bytes"];

/// Reads an append-only JSONL file into typed records.
pub fn read_json_lines<T>(path: &Path) -> io::Result<Vec<T>>
where
    T: DeserializeOwned,
{
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let parsed = serde_json::from_str::<T>(line.as_str()).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid JSONL test record: {error}"),
            )
        })?;
        records.push(parsed);
    }
    Ok(records)
}

/// Assertion failure for projection doctrine checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionAssertion {
    path: String,
    message: String,
}

impl ProjectionAssertion {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProjectionAssertion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for ProjectionAssertion {}

/// Asserts that a projection obeys the doctrine implied by its surface policy.
pub fn assert_projection_doctrine<T>(projection: &T) -> Result<(), ProjectionAssertion>
where
    T: ToolProjection,
{
    let concise = projection
        .concise_projection()
        .map_err(|error| ProjectionAssertion::new("$concise", error.to_string()))?;
    let full = projection
        .full_projection()
        .map_err(|error| ProjectionAssertion::new("$full", error.to_string()))?;
    let policy = projection.projection_policy();
    if policy.forbid_opaque_ids {
        assert_no_opaque_ids(&concise)?;
        assert_no_opaque_ids(&full)?;
    }
    if policy.reference_only {
        assert_reference_only(&concise)?;
        assert_reference_only(&full)?;
    }
    if matches!(policy.kind, SurfaceKind::List) {
        assert_list_shape(&concise)?;
        assert_list_shape(&full)?;
    }
    Ok(())
}

/// Asserts that a JSON value does not leak opaque database identifiers.
pub fn assert_no_opaque_ids(value: &Value) -> Result<(), ProjectionAssertion> {
    walk(value, "$", &mut |path, value| {
        if let Value::Object(object) = value {
            for key in object.keys() {
                if is_opaque_identifier_field(key) {
                    return Err(ProjectionAssertion::new(
                        format!("{path}.{key}"),
                        "opaque identifier field leaked into model-facing projection",
                    ));
                }
            }
        }
        Ok(())
    })
}

/// Asserts that a list-like surface does not inline prose bodies.
pub fn assert_list_shape(value: &Value) -> Result<(), ProjectionAssertion> {
    walk(value, "$", &mut |path, value| {
        if let Value::Object(object) = value {
            for key in object.keys() {
                if LIST_BODY_FIELDS.contains(&key.as_str()) {
                    return Err(ProjectionAssertion::new(
                        format!("{path}.{key}"),
                        "list surface leaked body-like content",
                    ));
                }
            }
        }
        Ok(())
    })
}

/// Asserts that a reference-only surface does not inline large textual content.
pub fn assert_reference_only(value: &Value) -> Result<(), ProjectionAssertion> {
    walk(value, "$", &mut |path, value| match value {
        Value::Object(object) => {
            for key in object.keys() {
                if REFERENCE_BODY_FIELDS.contains(&key.as_str()) {
                    return Err(ProjectionAssertion::new(
                        format!("{path}.{key}"),
                        "reference-only surface inlined artifact content",
                    ));
                }
            }
            Ok(())
        }
        _ => Ok(()),
    })
}

fn walk(
    value: &Value,
    path: &str,
    visitor: &mut impl FnMut(&str, &Value) -> Result<(), ProjectionAssertion>,
) -> Result<(), ProjectionAssertion> {
    visitor(path, value)?;
    match value {
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                walk(item, format!("{path}[{index}]").as_str(), visitor)?;
            }
        }
        Value::Object(object) => {
            for (key, child) in object {
                walk(child, format!("{path}.{key}").as_str(), visitor)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn is_opaque_identifier_field(key: &str) -> bool {
    key == OPAQUE_ID_FIELD || key.ends_with(OPAQUE_ID_SUFFIX)
}
