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
    /// The integer does not fit the requested machine type.
    #[error("integer is out of range")]
    OutOfRange,
}

/// A path-like value could not be normalized.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PathNormalizeError {
    /// The input was empty.
    #[error("path input must be non-empty")]
    Empty,
    /// Surrounding whitespace would make the path identity ambiguous.
    #[error("path input must not contain surrounding whitespace")]
    SurroundingWhitespace,
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
pub fn parse_human_unsigned_u64(raw: &str) -> Result<u64, NumericParseError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(NumericParseError::Empty);
    }
    if let Ok(value) = trimmed.parse::<u64>() {
        return Ok(value);
    }
    if !trimmed.contains('.') {
        return Err(if trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
            NumericParseError::OutOfRange
        } else {
            NumericParseError::Invalid
        });
    }
    let (integer, fraction) = trimmed.split_once('.').ok_or(NumericParseError::Invalid)?;
    if integer.is_empty() || fraction.is_empty() || !fraction.bytes().all(|byte| byte == b'0') {
        return Err(NumericParseError::Invalid);
    }
    integer
        .parse::<u64>()
        .map_err(|_| NumericParseError::OutOfRange)
}

/// Converts `u64` to the platform index width without saturation.
pub fn checked_u64_to_usize(value: u64) -> Result<usize, NumericParseError> {
    usize::try_from(value).map_err(|_| NumericParseError::OutOfRange)
}

/// Builds a lossy ASCII equivalence key for internal heuristic matching.
#[must_use]
pub(crate) fn fold_ascii_token(raw: &str) -> String {
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
    if raw.is_empty() {
        return Err(PathNormalizeError::Empty);
    }
    if raw.trim() != raw {
        return Err(PathNormalizeError::SurroundingWhitespace);
    }
    let parsed = if raw.starts_with("file://") {
        let file_url = Url::parse(raw).map_err(|_| PathNormalizeError::InvalidFileUri)?;
        file_url
            .to_file_path()
            .map_err(|()| PathNormalizeError::NonLocalFileUri)?
    } else {
        PathBuf::from(raw)
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
    use super::{
        NumericParseError, fold_ascii_token, normalize_local_path, parse_human_unsigned_u64,
    };
    use std::path::Path;

    #[test]
    fn parses_human_unsigned_integers() {
        assert_eq!(parse_human_unsigned_u64("42"), Ok(42));
        assert_eq!(parse_human_unsigned_u64("42.0"), Ok(42));
        assert_eq!(parse_human_unsigned_u64(" 7 "), Ok(7));
        assert_eq!(
            parse_human_unsigned_u64("-1"),
            Err(NumericParseError::Invalid)
        );
        assert_eq!(
            parse_human_unsigned_u64("7.5"),
            Err(NumericParseError::Invalid)
        );
        assert_eq!(
            parse_human_unsigned_u64("9007199254740993.0"),
            Ok(9_007_199_254_740_993)
        );
        assert_eq!(
            parse_human_unsigned_u64("18446744073709551616"),
            Err(NumericParseError::OutOfRange)
        );
    }

    #[test]
    fn normalizes_ascii_tokens() {
        assert_eq!(
            fold_ascii_token("textDocument/prepareRename"),
            "textdocumentpreparerename"
        );
        assert_eq!(fold_ascii_token("prepare_rename"), "preparerename");
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
        assert!(normalize_local_path(" src/lib.rs", Some(root)).is_err());
    }
}
