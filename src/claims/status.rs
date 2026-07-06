//! Claim and conflict status helpers.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::claims::card::ClaimCard;

/// Lifecycle status of a claim in assembled workspace state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    /// Active non-superseded claim.
    Current,
    /// Replaced by a newer claim.
    Superseded,
    /// Participates in an open conflict.
    Conflicting,
}

impl ClaimStatus {
    /// Serialize to the old status string form.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Superseded => "superseded",
            Self::Conflicting => "conflicting",
        }
    }
}

impl fmt::Display for ClaimStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Conflict record status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictStatus {
    /// Unresolved conflict.
    Open,
    /// Manually resolved.
    Resolved,
    /// Deliberately ignored.
    Ignored,
}

impl ConflictStatus {
    /// Serialize to the old status string form.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
            Self::Ignored => "ignored",
        }
    }
}

impl fmt::Display for ConflictStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Derive claim status with Superseded > Conflicting > Current precedence.
pub fn claim_status(card: &ClaimCard, in_open_conflict: bool) -> ClaimStatus {
    if card.phase == 2 {
        ClaimStatus::Superseded
    } else if in_open_conflict {
        ClaimStatus::Conflicting
    } else {
        ClaimStatus::Current
    }
}
