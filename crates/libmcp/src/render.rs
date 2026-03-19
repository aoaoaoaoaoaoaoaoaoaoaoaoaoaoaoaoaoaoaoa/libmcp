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
    let truncated = raw.chars().take(limit).collect::<String>();
    let visible_len = truncated.chars().count();
    if raw.chars().count() > visible_len {
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

/// Injects the common presentation controls into an object input schema.
#[must_use]
pub fn with_presentation_properties(schema: Value) -> Value {
    let Value::Object(mut object) = schema else {
        return schema;
    };
    let properties = object
        .entry("properties".to_owned())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Value::Object(properties) = properties {
        let _ = properties.insert("render".to_owned(), render_property_schema());
        let _ = properties.insert("detail".to_owned(), detail_property_schema());
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
        "description": "Output rendering. Defaults to porcelain for model-friendly summaries."
    })
}

fn detail_property_schema() -> Value {
    serde_json::json!({
        "type": "string",
        "enum": ["concise", "full"],
        "description": "Output detail level. Concise is the default model-facing summary; full retains more structure."
    })
}

/// Generic JSON-to-porcelain rendering configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonPorcelainConfig {
    /// Maximum output lines.
    pub max_lines: usize,
    /// Maximum inline characters for one preview fragment.
    pub max_inline_chars: usize,
}

impl Default for JsonPorcelainConfig {
    fn default() -> Self {
        Self {
            max_lines: 24,
            max_inline_chars: 120,
        }
    }
}

/// Renders arbitrary JSON into bounded, deterministic porcelain text.
#[must_use]
pub fn render_json_porcelain(value: &Value, config: JsonPorcelainConfig) -> String {
    let mut lines = Vec::<String>::new();
    render_top_level(value, config, &mut lines);
    if lines.is_empty() {
        return String::new();
    }
    if lines.len() > config.max_lines {
        lines.truncate(config.max_lines);
        let truncated_note = format!("... truncated to {} lines", config.max_lines);
        if let Some(last) = lines.last_mut() {
            *last = truncated_note;
        }
    }
    lines.join("\n")
}

fn render_top_level(value: &Value, config: JsonPorcelainConfig, lines: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if object.is_empty() {
                lines.push("empty object".to_owned());
                return;
            }
            let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
            keys.sort_unstable();
            for key in keys {
                let preview = inline_preview(&object[key], config);
                lines.push(format!("{key}: {preview}"));
            }
        }
        Value::Array(items) => {
            lines.push(format!("{} item(s)", items.len()));
            for (index, item) in items.iter().enumerate() {
                let preview = inline_preview(item, config);
                lines.push(format!("[{}] {preview}", index + 1));
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            lines.push(inline_preview(value, config));
        }
    }
}

fn inline_preview(value: &Value, config: JsonPorcelainConfig) -> String {
    let raw = match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => quote_string(text),
        Value::Array(items) => preview_array(items, config),
        Value::Object(object) => preview_object(object, config),
    };
    truncate_chars(raw.as_str(), Some(config.max_inline_chars)).text
}

fn preview_array(items: &[Value], config: JsonPorcelainConfig) -> String {
    if items.is_empty() {
        return "[]".to_owned();
    }
    let mut parts = items
        .iter()
        .take(3)
        .map(|item| inline_preview(item, config))
        .collect::<Vec<_>>();
    if items.len() > 3 {
        parts.push(format!("+{} more", items.len() - 3));
    }
    format!("[{}]", parts.join(", "))
}

fn preview_object(object: &serde_json::Map<String, Value>, config: JsonPorcelainConfig) -> String {
    if object.is_empty() {
        return "{}".to_owned();
    }
    let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
    keys.sort_unstable();
    let mut parts = keys
        .into_iter()
        .take(4)
        .map(|key| format!("{key}={}", inline_preview(&object[key], config)))
        .collect::<Vec<_>>();
    if object.len() > 4 {
        parts.push(format!("+{} more", object.len() - 4));
    }
    format!("{{{}}}", parts.join(", "))
}

fn quote_string(text: &str) -> String {
    format!("\"{}\"", collapse_inline_whitespace(text))
}

#[cfg(test)]
mod tests {
    use super::{
        DetailLevel, JsonPorcelainConfig, PathStyle, RenderConfig, RenderMode,
        collapse_inline_whitespace, render_json_porcelain, render_path,
        with_presentation_properties,
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
    fn renders_objects_and_arrays_to_bounded_porcelain() {
        let object = json!({
            "beta": {"nested": true, "count": 2},
            "alpha": "hello   world",
        });
        let rendered = render_json_porcelain(&object, JsonPorcelainConfig::default());
        assert_eq!(
            rendered,
            "alpha: \"hello world\"\nbeta: {count=2, nested=true}"
        );

        let array = json!([
            {"id": 1, "title": "first"},
            {"id": 2, "title": "second"},
        ]);
        let rendered = render_json_porcelain(&array, JsonPorcelainConfig::default());
        assert_eq!(
            rendered,
            "2 item(s)\n[1] {id=1, title=\"first\"}\n[2] {id=2, title=\"second\"}"
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
    }
}
