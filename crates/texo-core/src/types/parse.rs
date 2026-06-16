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
    /// Source-hash input too short to derive an id.
    #[error("body hash must be at least {min} hex characters")]
    HashTooShort {
        /// Minimum required hex character count.
        min: usize,
    },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expect_prefix_rejects_empty() {
        assert_eq!(expect_prefix("", "claim_"), Err(IdParseError::Empty));
    }

    #[test]
    fn expect_prefix_rejects_wrong_prefix() {
        assert_eq!(
            expect_prefix("src_abc", "claim_"),
            Err(IdParseError::BadPrefix {
                expected: "claim_".to_string()
            })
        );
    }

    #[test]
    fn expect_prefix_accepts_matching_prefix() {
        assert_eq!(
            expect_prefix("claim_abc", "claim_").expect("ok"),
            "claim_abc".to_string()
        );
    }

    #[test]
    fn validate_workspace_rejects_unsafe_characters() {
        for bad in ["", "a/b", "a\\b", "a\0b"] {
            // Each unsafe segment must be rejected with InvalidWorkspace.
            assert_eq!(validate_workspace(bad), Err(IdParseError::InvalidWorkspace));
        }
    }

    #[test]
    fn validate_workspace_accepts_safe_segment() {
        assert_eq!(validate_workspace("demo-workspace_1"), Ok(()));
    }

    #[test]
    fn error_display_messages_are_diagnosable() {
        assert_eq!(
            IdParseError::Empty.to_string(),
            "identifier must not be empty"
        );
        assert_eq!(
            IdParseError::BadPrefix {
                expected: "claim_".to_string()
            }
            .to_string(),
            "identifier must start with `claim_`"
        );
        assert_eq!(
            IdParseError::InvalidWorkspace.to_string(),
            "invalid workspace id"
        );
        assert_eq!(
            IdParseError::HashTooShort { min: 12 }.to_string(),
            "body hash must be at least 12 hex characters"
        );
    }
}
