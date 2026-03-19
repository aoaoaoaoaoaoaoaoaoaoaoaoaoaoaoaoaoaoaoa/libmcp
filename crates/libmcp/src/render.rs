//! Model-facing rendering helpers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
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
    /// Chosen path rendering style.
    pub path_style: PathStyle,
}

impl RenderConfig {
    /// Builds a render configuration from user input, applying the default
    /// path style implied by the render mode.
    #[must_use]
    pub fn from_user_input(render: Option<RenderMode>, path_style: Option<PathStyle>) -> Self {
        let render = render.unwrap_or(RenderMode::Porcelain);
        let default_path_style = match render {
            RenderMode::Porcelain => PathStyle::Relative,
            RenderMode::Json => PathStyle::Absolute,
        };
        Self {
            render,
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

#[cfg(test)]
mod tests {
    use super::{PathStyle, RenderConfig, RenderMode, collapse_inline_whitespace, render_path};
    use std::path::Path;

    #[test]
    fn render_config_uses_mode_specific_defaults() {
        let porcelain = RenderConfig::from_user_input(None, None);
        assert_eq!(porcelain.render, RenderMode::Porcelain);
        assert_eq!(porcelain.path_style, PathStyle::Relative);

        let json = RenderConfig::from_user_input(Some(RenderMode::Json), None);
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
}
