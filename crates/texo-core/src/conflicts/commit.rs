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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::state::ClaimView;
    use crate::types::ids::{ClaimId, SourceId};
    use crate::types::receipt::receipt_view;
    use crate::types::status::ClaimStatus;

    fn deploy_claim(id: &str, day: &str) -> ClaimView {
        ClaimView {
            claim_id: ClaimId::try_from(id).expect("claim id"),
            workspace_id: "demo".to_string(),
            source_id: SourceId::try_from("src_abc123def456").expect("source id"),
            source_path: "schedule.md".to_string(),
            line_start: 1,
            line_end: 1,
            text: format!("Deploy window is {day}."),
            normalized_text: format!("deploy window is {day}"),
            subject_hint: "deploy-process".to_string(),
            predicate_hint: "unknown".to_string(),
            object_hint: day.to_string(),
            confidence_ppm: 600_000,
            extractor_kind: "test".to_string(),
            status: ClaimStatus::Current,
            receipt: receipt_view(1, 1, "ClaimRecorded", "workspace:demo", id),
            supersedes: Vec::new(),
            superseded_by: None,
        }
    }

    #[test]
    fn conflict_report_json_serializes_detected_report() {
        // Build a real report through the detection path (Friday vs Tuesday on
        // the deploy-process subject) and serialize it via the public helper.
        let a = deploy_claim("claim_aaaaaaaaaaaa", "friday");
        let b = deploy_claim("claim_bbbbbbbbbbbb", "tuesday");
        let mut state = ClaimState::default();
        state.claims.insert(a.claim_id.clone(), a);
        state.claims.insert(b.claim_id.clone(), b);
        let workspace = WorkspaceId::new("demo").expect("workspace");
        let report = detect_conflicts(&state, &workspace);
        // The Friday/Tuesday deploy fixture detects exactly one conflict.
        assert_eq!(report.conflicts.len(), 1);

        let json = conflict_report_json(&report).expect("serialize report");
        // The JSON must round-trip back into an identical report value.
        let parsed: ConflictReport = serde_json::from_str(&json).expect("parse report json");
        assert_eq!(parsed.workspace_id.as_str(), "demo");
        assert_eq!(parsed.conflicts.len(), 1);
        let entry = &parsed.conflicts[0];
        assert_eq!(entry.subject_hint, "deploy-process");
        assert_eq!(entry.status, ConflictStatus::Open);
        let mut pair = [entry.claim_a.as_str(), entry.claim_b.as_str()];
        pair.sort_unstable();
        assert_eq!(pair, ["claim_aaaaaaaaaaaa", "claim_bbbbbbbbbbbb"]);
    }
}
