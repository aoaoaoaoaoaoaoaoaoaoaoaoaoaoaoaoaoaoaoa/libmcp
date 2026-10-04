//! Model-facing rendering helpers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// Output render mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum RenderMode {
    /// Model-optimized text output.
    #[default]
    #[serde(alias = "text", alias = "plain", alias = "plain_text")]
    Porcelain,
    /// Structured JSON output.
    #[serde(alias = "structured")]
    Json,
}

/// Output detail level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum DetailLevel {
    /// Model-optimized concise output.
    #[default]
    #[serde(alias = "summary", alias = "compact")]
    Concise,
    /// Verbose output that retains additional structure and fields.
    #[serde(alias = "verbose", alias = "detailed")]
    Full,
}

/// Path rendering style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum PathStyle {
    /// Render absolute filesystem paths.
    Absolute,
    /// Render paths relative to the workspace root when possible.
    #[default]
    #[serde(alias = "rel")]
    Relative,
}

/// Common render configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderConfig {
    /// Chosen render mode.
    pub render: RenderMode,
    /// Chosen detail level.
    pub detail: DetailLevel,
    /// Chosen path rendering style.
    pub path_style: PathStyle,
}

impl RenderConfig {
    /// Builds a render configuration from user input, applying the default
    /// path style implied by the render mode.
    #[must_use]
    pub fn from_user_input(
        render: Option<RenderMode>,
        path_style: Option<PathStyle>,
        detail: Option<DetailLevel>,
    ) -> Self {
        let render = render.unwrap_or(RenderMode::Porcelain);
        let default_path_style = match render {
            RenderMode::Porcelain => PathStyle::Relative,
            RenderMode::Json => PathStyle::Absolute,
        };
        Self {
            render,
            detail: detail.unwrap_or(DetailLevel::Concise),
            path_style: path_style.unwrap_or(default_path_style),
        }
    }
}

/// Result of text truncation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TruncatedText {
    /// Visible text after truncation.
    pub text: String,
    /// Whether text was truncated.
    pub truncated: bool,
}

/// Collapses all internal whitespace runs into single spaces.
#[must_use]
pub fn collapse_inline_whitespace(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncates a string by Unicode scalar count.
#[must_use]
pub fn truncate_chars(raw: &str, limit: Option<usize>) -> TruncatedText {
    let Some(limit) = limit else {
        return TruncatedText {
            text: raw.to_owned(),
            truncated: false,
        };
    };
    let mut characters = raw.chars();
    let truncated = characters.by_ref().take(limit).collect::<String>();
    if characters.next().is_some() {
        TruncatedText {
            text: truncated,
            truncated: true,
        }
    } else {
        TruncatedText {
            text: raw.to_owned(),
            truncated: false,
        }
    }
}

/// Renders a path according to the requested style.
#[must_use]
pub fn render_path(path: &Path, style: PathStyle, workspace_root: Option<&Path>) -> String {
    match style {
        PathStyle::Absolute => path.display().to_string(),
        PathStyle::Relative => {
            if let Some(workspace_root) = workspace_root
                && let Ok(relative) = path.strip_prefix(workspace_root)
            {
                return relative.display().to_string();
            }
            path.display().to_string()
        }
    }
}

/// Adds the conventional concise/full control without replacing tool-owned controls.
#[must_use]
pub fn with_presentation_properties(schema: Value) -> Value {
    with_control(
        with_render_property(schema),
        "detail",
        detail_property_schema(),
    )
}

/// Adds only rendering; content controls remain tool-owned.
#[must_use]
pub fn with_render_property(schema: Value) -> Value {
    with_control(schema, "render", render_property_schema())
}

fn with_control(schema: Value, name: &str, control: Value) -> Value {
    let Value::Object(mut object) = schema else {
        return schema;
    };
    let properties = object
        .entry("properties".to_owned())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Value::Object(properties) = properties {
        let _ = properties.entry(name.to_owned()).or_insert(control);
    }
    let _ = object
        .entry("additionalProperties".to_owned())
        .or_insert(Value::Bool(false));
    Value::Object(object)
}

fn render_property_schema() -> Value {
    serde_json::json!({
        "type": "string",
        "enum": ["porcelain", "json"],
        "default": "porcelain",
        "description": "Encoding only; defaults to porcelain."
    })
}

fn detail_property_schema() -> Value {
    serde_json::json!({
        "type": "string",
        "enum": ["concise", "full"],
        "default": "concise",
        "description": "Selected content: concise (default) or full. Independent of render."
    })
}

/// Encodes the complete selected value as text. Bounds belong to content selection.
///
/// Short homogeneous records use tables; other collections use indentation.
/// Ambiguous scalars are quoted, and multiline text uses literal blocks.
#[must_use]
pub fn render_json_porcelain(value: &Value) -> String {
    let mut lines = Vec::new();
    render_value(value, 0, "", &mut lines);
    // Literal blocks need their final line ending to preserve trailing newlines.
    lines.push(String::new());
    lines.join("\n")
}

fn render_value(value: &Value, depth: usize, label: &str, lines: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    let prefix = format!("{indent}{label}");
    match value {
        Value::Object(fields) if !fields.is_empty() => {
            let child_depth = if label.is_empty() {
                depth
            } else {
                lines.push(prefix.trim_end().to_owned());
                depth + 1
            };
            for (key, value) in fields {
                render_value(
                    value,
                    child_depth,
                    &format!("{}: ", render_string(key)),
                    lines,
                );
            }
        }
        Value::Array(items) if !items.is_empty() => {
            if let Some(keys) = table_keys(items) {
                lines.push(format!(
                    "{prefix}table[{}]: {}",
                    items.len(),
                    keys.iter()
                        .map(|key| render_string(key))
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
                for item in items {
                    lines.push(format!(
                        "{indent}  {}",
                        keys.iter()
                            .map(|key| render_scalar(&item[*key]))
                            .collect::<Vec<_>>()
                            .join(" | ")
                    ));
                }
            } else {
                let child_depth = if label.is_empty() {
                    depth
                } else {
                    lines.push(prefix.trim_end().to_owned());
                    depth + 1
                };
                for item in items {
                    render_value(item, child_depth, "- ", lines);
                }
            }
        }
        Value::String(text)
            if text.contains('\n')
                && !text.chars().any(|c| {
                    (c.is_control() && c != '\n' && c != '\t')
                        || matches!(c, '\u{2028}' | '\u{2029}')
                }) =>
        {
            let chomping = if text.ends_with("\n\n") || text == "\n" {
                "+"
            } else if text.ends_with('\n') {
                ""
            } else {
                "-"
            };
            lines.push(format!("{prefix}|2{chomping}"));
            for line in text.split_terminator('\n') {
                lines.push(format!("{indent}  {line}"));
            }
        }
        _ => lines.push(format!("{prefix}{}", render_scalar(value))),
    }
}

fn table_keys(items: &[Value]) -> Option<Vec<&str>> {
    let first = items.first()?.as_object()?;
    // These thresholds choose layout, never content. Wide or nested rows fall
    // back to complete records rather than clipping cells or columns.
    if items.len() < 2 || first.is_empty() || first.len() > 12 {
        return None;
    }
    items
        .iter()
        .all(|item| {
            item.as_object().is_some_and(|row| {
                row.keys().eq(first.keys())
                    && row.values().all(|value| {
                        !value.is_array()
                            && !value.is_object()
                            && render_scalar(value).chars().count() <= 96
                    })
            })
        })
        .then(|| first.keys().map(String::as_str).collect())
}

fn render_scalar(value: &Value) -> String {
    match value {
        Value::String(text) => render_string(text),
        _ => value.to_string(),
    }
}

fn render_string(text: &str) -> String {
    let plain = text
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && text.trim() == text
        && !text
            .chars()
            .any(|c| c.is_control() || ":[]{}|\"'\\#\u{2028}\u{2029}".contains(c))
        && !matches!(
            text.to_ascii_lowercase().as_str(),
            "null" | "true" | "false" | "yes" | "no" | "on" | "off" | "y" | "n"
        );
    if plain {
        text.to_owned()
    } else {
        // JSON permits these raw characters, but text readers may interpret
        // them as line breaks and fold away their identity.
        Value::String(text.to_owned())
            .to_string()
            .replace('\u{0085}', "\\u0085")
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DetailLevel, PathStyle, RenderConfig, RenderMode, collapse_inline_whitespace,
        render_json_porcelain, render_path, with_presentation_properties, with_render_property,
    };
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn render_config_uses_mode_specific_defaults() {
        let porcelain = RenderConfig::from_user_input(None, None, None);
        assert_eq!(porcelain.render, RenderMode::Porcelain);
        assert_eq!(porcelain.detail, DetailLevel::Concise);
        assert_eq!(porcelain.path_style, PathStyle::Relative);

        let json = RenderConfig::from_user_input(Some(RenderMode::Json), None, None);
        assert_eq!(json.path_style, PathStyle::Absolute);
    }

    #[test]
    fn collapses_whitespace_and_renders_relative_paths() {
        assert_eq!(collapse_inline_whitespace("a   b\t c"), "a b c");
        let root = Path::new("/tmp/repo");
        let path = Path::new("/tmp/repo/src/lib.rs");
        assert_eq!(
            render_path(path, PathStyle::Relative, Some(root)),
            "src/lib.rs"
        );
    }

    #[test]
    fn renders_complete_records_and_unambiguous_table_cells() {
        let object = json!({
            "beta": {"nested": true, "count": 2},
            "alpha": "hello   world",
        });
        let rendered = render_json_porcelain(&object);
        assert_eq!(
            rendered,
            "alpha: hello   world\nbeta:\n  count: 2\n  nested: true\n"
        );

        let array = json!([
            {"id": 1, "title": "first"},
            {"id": 2, "title": "true"},
            {"id": 3, "title": "x | y"},
            {"id": 4, "title": null},
            {"id": 5, "title": "null"},
        ]);
        let rendered = render_json_porcelain(&array);
        assert_eq!(
            rendered,
            "table[5]: id | title\n  1 | first\n  2 | \"true\"\n  3 | \"x | y\"\n  4 | null\n  5 | \"null\"\n"
        );
    }

    #[test]
    fn injects_render_and_detail_schema_properties() {
        let schema = with_presentation_properties(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            }
        }));
        assert_eq!(
            schema["properties"]["render"]["enum"],
            json!(["porcelain", "json"])
        );
        assert_eq!(
            schema["properties"]["detail"]["enum"],
            json!(["concise", "full"])
        );
        assert_eq!(schema["additionalProperties"], json!(false));
        let custom = json!({"type":"object","properties":{"detail":{"enum":["headers","body"]}}});
        assert_eq!(
            with_presentation_properties(custom.clone())["properties"]["detail"],
            custom["properties"]["detail"]
        );
        let render_only = with_render_property(json!({"type":"object"}));
        assert!(render_only["properties"].get("detail").is_none());
    }

    #[test]
    fn preserves_selected_text_and_deep_collections() {
        // Format-owned limits used to hide long strings and nested values even
        // at full detail. Selection alone may omit facts, not either encoder.
        let text = format!("{}leaf", "λ".repeat(2048));
        let mut nested =
            json!([0,1,2,3,{"fifth":text.clone(),"empty":{},"list":[],"null":null,"last":"tail"}]);
        for _depth in 0..32 {
            nested = json!({"next": nested});
        }
        let nested = render_json_porcelain(&nested);
        assert!(nested.contains(&text));
        assert!(nested.contains("last: tail"));
        assert!(nested.contains("empty: {}"));
        assert!(nested.contains("list: []"));
        assert!(nested.contains("\"null\": null"));
        assert_eq!(
            render_json_porcelain(&json!("a  b\n  c")),
            "|2-\n  a  b\n    c\n"
        );
        assert_eq!(render_json_porcelain(&json!("a\n\n")), "|2+\n  a\n  \n");
        assert_eq!(render_json_porcelain(&json!("\n")), "|2+\n  \n");
        for text in ["x\u{0085}y", "x\u{2028}y", "x\u{2029}y"] {
            let encoded = render_json_porcelain(&json!(text));
            assert_eq!(
                serde_json::from_str::<String>(&encoded).ok().as_deref(),
                Some(text)
            );
            assert!(encoded.contains("\\u"));
        }
        for text in ["", "false", "null", "123", " x ", "x|y", "\r\n"] {
            assert_eq!(
                render_json_porcelain(&json!(text)).trim_end(),
                json!(text).to_string()
            );
        }
    }
}
