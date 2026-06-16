//! Commit detected conflicts to the journal.

use crate::conflicts::detect::detect_conflicts;
use crate::events::payloads::ClaimConflictDetected;
use crate::journal::store::StoreHandle;
use crate::journal::JournalError;
use crate::replay::state::ClaimState;
use crate::state::conflict_lifecycle::{CommittedConflict, ConflictReport};
use crate::types::ids::WorkspaceId;
use crate::types::status::ConflictStatus;

/// Append open conflicts from a read-only report.
pub fn commit_conflicts(
    handle: &StoreHandle,
    state: &ClaimState,
    workspace_id: &WorkspaceId,
    observed_at_ms: u64,
) -> Result<Vec<CommittedConflict>, JournalError> {
    let report = detect_conflicts(state, workspace_id);
    let mut committed = Vec::new();
    for entry in report.conflicts {
        if state.conflicts.contains_key(&entry.conflict_id) {
            continue;
        }
        let payload = ClaimConflictDetected {
            conflict_id: entry.conflict_id.to_string(),
            workspace_id: workspace_id.to_string(),
            claim_a: entry.claim_a.to_string(),
            claim_b: entry.claim_b.to_string(),
            reason: entry.reason.clone(),
            status: ConflictStatus::Open.as_str().to_string(),
            observed_at_ms,
        };
        let receipt = handle.append_conflict(&payload)?;
        committed.push(CommittedConflict {
            conflict_id: entry.conflict_id,
            sequence: receipt.sequence.get(),
        });
    }
    Ok(committed)
}

/// Serialize conflict report to JSON file shape.
pub fn conflict_report_json(report: &ConflictReport) -> serde_json::Result<String> {
    serde_json::to_string_pretty(report)
}
