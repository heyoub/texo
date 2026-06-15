//! Conflict lifecycle types.

use crate::types::ids::{ClaimId, ConflictId};
use crate::types::status::ConflictStatus;

/// Read-only conflict report entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConflictEntry {
    /// Conflict id.
    pub conflict_id: ConflictId,
    /// First claim.
    pub claim_a: ClaimId,
    /// Second claim.
    pub claim_b: ClaimId,
    /// Subject hint shared by both claims.
    pub subject_hint: String,
    /// Heuristic reason.
    pub reason: String,
    /// Current status.
    pub status: ConflictStatus,
}

/// Read-only conflict detection report.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConflictReport {
    /// Workspace id.
    pub workspace_id: String,
    /// Detected conflicts.
    pub conflicts: Vec<ConflictEntry>,
}

/// Result of committing conflicts to the journal.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CommittedConflict {
    /// Conflict id.
    pub conflict_id: ConflictId,
    /// Receipt sequence.
    pub sequence: u64,
}
