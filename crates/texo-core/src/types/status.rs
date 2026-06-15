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

impl ConflictStatus {
    /// Parse status from journal event string.
    pub fn parse_str(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "resolved" => Some(Self::Resolved),
            "ignored" => Some(Self::Ignored),
            _ => None,
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
