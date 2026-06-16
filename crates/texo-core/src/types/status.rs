//! Claim and conflict status enums.

use serde::{Deserialize, Serialize};

/// Lifecycle status of a claim in replayed state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ClaimStatus {
    /// Active non-superseded claim.
    Current,
    /// Replaced by a newer claim.
    Superseded,
    /// Participates in an open conflict.
    Conflicting,
    /// Status not yet determined.
    Unknown,
}

/// Conflict record status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ConflictStatus {
    /// Unresolved conflict.
    Open,
    /// Manually resolved.
    Resolved,
    /// Deliberately ignored.
    Ignored,
}

/// Unrecognized conflict status string encountered while parsing a journal event.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{value}")]
pub struct ConflictStatusParseError {
    /// The offending status string that did not match a known conflict status.
    pub value: String,
}

impl ConflictStatus {
    /// Parse status from journal event string.
    pub fn parse_str(value: &str) -> Result<Self, ConflictStatusParseError> {
        match value {
            "open" => Ok(Self::Open),
            "resolved" => Ok(Self::Resolved),
            "ignored" => Ok(Self::Ignored),
            _ => Err(ConflictStatusParseError {
                value: value.to_string(),
            }),
        }
    }

    /// Serialize to journal event string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
            Self::Ignored => "ignored",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_status_parse_accepts_every_known_string() {
        assert_eq!(
            ConflictStatus::parse_str("open").expect("open"),
            ConflictStatus::Open
        );
        assert_eq!(
            ConflictStatus::parse_str("resolved").expect("resolved"),
            ConflictStatus::Resolved
        );
        assert_eq!(
            ConflictStatus::parse_str("ignored").expect("ignored"),
            ConflictStatus::Ignored
        );
    }

    #[test]
    fn conflict_status_parse_rejects_unknown_string() {
        let err = ConflictStatus::parse_str("bogus").expect_err("unknown must error");
        assert_eq!(err.value, "bogus");
        // The Display impl must surface the offending value verbatim so a
        // malformed journal entry is diagnosable.
        assert_eq!(err.to_string(), "bogus");
    }

    #[test]
    fn conflict_status_round_trips_through_as_str_and_parse() {
        for status in [
            ConflictStatus::Open,
            ConflictStatus::Resolved,
            ConflictStatus::Ignored,
        ] {
            assert_eq!(
                ConflictStatus::parse_str(status.as_str()).expect("round trip"),
                status
            );
        }
    }

    #[test]
    fn claim_status_serde_uses_snake_case() {
        // Golden serde shape: the journal/JSON surface must stay snake_case.
        let json = serde_json::to_string(&ClaimStatus::Conflicting).expect("serialize");
        assert_eq!(json, "\"conflicting\"");
        let parsed: ClaimStatus = serde_json::from_str("\"superseded\"").expect("deserialize");
        assert_eq!(parsed, ClaimStatus::Superseded);
    }
}
