//! Identifier parsing and validation.

use thiserror::Error;

/// Failure to parse a branded identifier.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdParseError {
    /// Empty identifier.
    #[error("identifier must not be empty")]
    Empty,
    /// Missing required prefix.
    #[error("identifier must start with `{expected}`")]
    BadPrefix {
        /// Expected prefix.
        expected: String,
    },
    /// Invalid workspace characters.
    #[error("invalid workspace id")]
    InvalidWorkspace,
}

pub(crate) fn expect_prefix(value: &str, prefix: &str) -> Result<String, IdParseError> {
    if value.is_empty() {
        return Err(IdParseError::Empty);
    }
    if !value.starts_with(prefix) {
        return Err(IdParseError::BadPrefix {
            expected: prefix.to_string(),
        });
    }
    Ok(value.to_string())
}

pub(crate) fn validate_workspace(value: &str) -> Result<(), IdParseError> {
    if value.is_empty() || value.contains('/') || value.contains('\\') || value.contains('\0') {
        return Err(IdParseError::InvalidWorkspace);
    }
    Ok(())
}
