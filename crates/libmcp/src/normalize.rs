//! Shared normalization helpers for model-facing input.

use std::path::{Path, PathBuf};
use thiserror::Error;
use url::Url;

/// A numeric input could not be normalized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum NumericParseError {
    /// The input was empty.
    #[error("numeric input must be non-empty")]
    Empty,
    /// The input could not be represented as a non-negative integer.
    #[error("expected a non-negative integer")]
    Invalid,
}

/// A path-like value could not be normalized.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PathNormalizeError {
    /// The input was empty.
    #[error("path input must be non-empty")]
    Empty,
    /// The `file://` URI was malformed.
    #[error("file URI is invalid")]
    InvalidFileUri,
    /// The URI does not reference a local path.
    #[error("file URI must resolve to a local path")]
    NonLocalFileUri,
}

/// Parses a human-facing unsigned integer.
///
/// This accepts:
///
/// - integer numbers
/// - integer-like floating-point spellings such as `42.0`
/// - numeric strings
#[must_use]
pub fn parse_human_unsigned_u64(raw: &str) -> Option<u64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = trimmed.parse::<u64>() {
        return Some(value);
    }
    let parsed_float = trimmed.parse::<f64>().ok()?;
    if !parsed_float.is_finite() || parsed_float < 0.0 || parsed_float.fract() != 0.0 {
        return None;
    }
    let max = u64::MAX as f64;
    if parsed_float > max {
        return None;
    }
    Some(parsed_float as u64)
}

/// Converts `u64` to `usize`, saturating on overflow.
#[must_use]
pub fn saturating_u64_to_usize(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// Normalizes a token by dropping non-alphanumeric ASCII and lowercasing.
#[must_use]
pub fn normalize_ascii_token(raw: &str) -> String {
    raw.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

/// Resolves a local path or `file://` URI to an absolute path.
pub fn normalize_local_path(
    raw: &str,
    workspace_root: Option<&Path>,
) -> Result<PathBuf, PathNormalizeError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(PathNormalizeError::Empty);
    }
    let parsed = if trimmed.starts_with("file://") {
        let file_url = Url::parse(trimmed).map_err(|_| PathNormalizeError::InvalidFileUri)?;
        file_url
            .to_file_path()
            .map_err(|()| PathNormalizeError::NonLocalFileUri)?
    } else {
        PathBuf::from(trimmed)
    };
    Ok(if parsed.is_absolute() {
        parsed
    } else if let Some(workspace_root) = workspace_root {
        workspace_root.join(parsed)
    } else {
        parsed
    })
}

#[cfg(test)]
mod tests {
    use super::{normalize_ascii_token, normalize_local_path, parse_human_unsigned_u64};
    use std::path::Path;

    #[test]
    fn parses_human_unsigned_integers() {
        assert_eq!(parse_human_unsigned_u64("42"), Some(42));
        assert_eq!(parse_human_unsigned_u64("42.0"), Some(42));
        assert_eq!(parse_human_unsigned_u64(" 7 "), Some(7));
        assert_eq!(parse_human_unsigned_u64("-1"), None);
        assert_eq!(parse_human_unsigned_u64("7.5"), None);
    }

    #[test]
    fn normalizes_ascii_tokens() {
        assert_eq!(
            normalize_ascii_token("textDocument/prepareRename"),
            "textdocumentpreparerename"
        );
        assert_eq!(normalize_ascii_token("prepare_rename"), "preparerename");
    }

    #[test]
    fn resolves_relative_paths_against_workspace_root() {
        let root = Path::new("/tmp/example-root");
        let resolved = normalize_local_path("src/lib.rs", Some(root));
        assert!(resolved.is_ok());
        assert_eq!(
            resolved.ok().as_deref(),
            Some(root.join("src/lib.rs").as_path())
        );
    }
}
