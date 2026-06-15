//! Read-only conflict detection.

use crate::replay::state::{ClaimState, ClaimView};
use crate::stale::check::has_replacement_keyword;
use crate::state::conflict_lifecycle::{ConflictEntry, ConflictReport};
use crate::types::ids::{conflict_id_from_pair, ClaimId};
use crate::types::status::{ClaimStatus, ConflictStatus};

const CONTRADICTION_SUBJECTS: &[&str] = &[
    "deploy-process",
    "release-process",
    "approval-process",
    "ownership",
];

/// Detect conflicts from current replayed state (read-only).
pub fn detect_conflicts(state: &ClaimState, workspace_id: &str) -> ConflictReport {
    let mut by_subject: std::collections::HashMap<String, Vec<&ClaimView>> =
        std::collections::HashMap::new();
    for claim in state
        .claims
        .values()
        .filter(|c| c.status == ClaimStatus::Current)
    {
        by_subject
            .entry(claim.subject_hint.clone())
            .or_default()
            .push(claim);
    }

    let mut conflicts = Vec::new();
    for (subject, group) in by_subject {
        if group.len() < 2 {
            continue;
        }
        for i in 0..group.len() {
            for j in (i + 1)..group.len() {
                let a = group[i];
                let b = group[j];
                if a.normalized_text == b.normalized_text {
                    continue;
                }
                if already_superseded_pair(state, &a.claim_id, &b.claim_id) {
                    continue;
                }
                if !has_contradiction_signal(a, b, &subject) {
                    continue;
                }
                let conflict_id = conflict_id_from_pair(&a.claim_id, &b.claim_id);
                conflicts.push(ConflictEntry {
                    conflict_id,
                    claim_a: a.claim_id.clone(),
                    claim_b: b.claim_id.clone(),
                    subject_hint: subject.clone(),
                    reason: format!(
                        "contradictory current claims on {subject}: \"{}\" vs \"{}\"",
                        a.text, b.text
                    ),
                    status: ConflictStatus::Open,
                });
            }
        }
    }

    ConflictReport {
        workspace_id: workspace_id.to_string(),
        conflicts,
    }
}

fn already_superseded_pair(state: &ClaimState, a: &ClaimId, b: &ClaimId) -> bool {
    state.superseded.contains_key(a.as_str()) || state.superseded.contains_key(b.as_str())
}

fn has_contradiction_signal(a: &ClaimView, b: &ClaimView, subject: &str) -> bool {
    if has_replacement_keyword(&a.text)
        || has_replacement_keyword(&a.normalized_text)
        || has_replacement_keyword(&b.text)
        || has_replacement_keyword(&b.normalized_text)
    {
        return true;
    }
    if negation_mismatch(a, b) {
        return true;
    }
    if a.predicate_hint != b.predicate_hint
        && (a.predicate_hint == "changed" || b.predicate_hint == "changed")
    {
        return true;
    }
    if a.predicate_hint == "owns" && b.predicate_hint == "owns" && a.object_hint != b.object_hint {
        return true;
    }
    if schedule_object_clash(a, b, subject) {
        return true;
    }
    if confidence_gap(a, b) {
        return true;
    }
    if CONTRADICTION_SUBJECTS.contains(&subject)
        && a.object_hint != b.object_hint
        && !a.object_hint.is_empty()
        && !b.object_hint.is_empty()
    {
        return true;
    }
    false
}

fn negation_mismatch(a: &ClaimView, b: &ClaimView) -> bool {
    let a_neg = a.normalized_text.contains(" not ") || a.normalized_text.starts_with("not ");
    let b_neg = b.normalized_text.contains(" not ") || b.normalized_text.starts_with("not ");
    if a_neg == b_neg {
        return false;
    }
    let strip = |s: &str| s.replace(" not ", " ").replace("not ", "");
    strip(&a.normalized_text) == strip(&b.normalized_text)
}

const SCHEDULE_DAYS: &[&str] = &["monday", "tuesday", "wednesday", "thursday", "friday"];

fn schedule_object_clash(a: &ClaimView, b: &ClaimView, subject: &str) -> bool {
    if subject != "deploy-process" && subject != "release-process" {
        return false;
    }
    let a_day = SCHEDULE_DAYS.iter().find(|d| a.object_hint.contains(**d));
    let b_day = SCHEDULE_DAYS.iter().find(|d| b.object_hint.contains(**d));
    matches!((a_day, b_day), (Some(da), Some(db)) if da != db)
}

fn confidence_gap(a: &ClaimView, b: &ClaimView) -> bool {
    const THRESHOLD: u32 = 250_000;
    a.confidence_ppm.abs_diff(b.confidence_ppm) > THRESHOLD
        && a.normalized_text != b.normalized_text
}

/// Verify replayed projection consistency.
pub fn verify_projection(state: &ClaimState) -> Result<(), VerifyError> {
    if state.replayed_through_sequence == 0 && !state.claims.is_empty() {
        return Err(VerifyError::Projection(
            "frontier must be non-zero when claims exist".to_string(),
        ));
    }
    for claim in state.claims.values() {
        if !state.sources.contains_key(claim.source_id.as_str()) {
            return Err(VerifyError::Projection(format!(
                "claim {} references unknown source {}",
                claim.claim_id, claim.source_id
            )));
        }
    }
    for sup in state.superseded.values() {
        if !state.claims.contains_key(sup.old_claim_id.as_str()) {
            return Err(VerifyError::Projection(format!(
                "supersession references unknown old claim {}",
                sup.old_claim_id
            )));
        }
        if !state.claims.contains_key(sup.new_claim_id.as_str()) {
            return Err(VerifyError::Projection(format!(
                "supersession references unknown new claim {}",
                sup.new_claim_id
            )));
        }
    }
    Ok(())
}

/// Deprecated alias for [`verify_projection`].
pub fn verify_store(state: &ClaimState) -> Result<(), String> {
    verify_projection(state).map_err(|e| e.to_string())
}

/// Verification errors for projection and journal receipts.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VerifyError {
    /// Projection invariant violation.
    #[error("projection: {0}")]
    Projection(String),
    /// Journal receipt verification failure.
    #[error("journal: {0}")]
    Journal(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::state::ClaimView;
    use crate::types::ids::SourceId;
    use crate::types::receipt::receipt_view;

    fn claim(id: &str, subject: &str, text: &str, predicate: &str, object: &str) -> ClaimView {
        ClaimView {
            claim_id: ClaimId::try_from(id).expect("id"),
            workspace_id: "demo".to_string(),
            source_id: SourceId::try_from("src_abc123def456").expect("id"),
            source_path: "x.md".to_string(),
            line_start: 1,
            line_end: 1,
            text: text.to_string(),
            normalized_text: text.to_ascii_lowercase(),
            subject_hint: subject.to_string(),
            predicate_hint: predicate.to_string(),
            object_hint: object.to_string(),
            confidence_ppm: 650_000,
            extractor_kind: "test".to_string(),
            status: ClaimStatus::Current,
            receipt: receipt_view(1, 1, "ClaimRecorded", "workspace:demo", id),
            supersedes: Vec::new(),
            superseded_by: None,
        }
    }

    #[test]
    fn negation_pair_detects_conflict() {
        let a = claim(
            "claim_aaaaaaaaaaaa",
            "deploy-process",
            "Deploys must happen on Friday.",
            "must",
            "friday",
        );
        let b = claim(
            "claim_bbbbbbbbbbbb",
            "deploy-process",
            "Deploys must not happen on Friday.",
            "must",
            "friday",
        );
        let mut state = ClaimState::default();
        state.claims.insert(a.claim_id.to_string(), a.clone());
        state.claims.insert(b.claim_id.to_string(), b);
        let report = detect_conflicts(&state, "demo");
        assert_eq!(report.conflicts.len(), 1);
    }

    #[test]
    fn schedule_object_clash_without_keyword() {
        let a = claim(
            "claim_aaaaaaaaaaaa",
            "deploy-process",
            "Deploy window is Friday.",
            "unknown",
            "friday",
        );
        let b = claim(
            "claim_bbbbbbbbbbbb",
            "deploy-process",
            "Deploy window is Tuesday.",
            "unknown",
            "tuesday",
        );
        let mut state = ClaimState::default();
        state.claims.insert(a.claim_id.to_string(), a.clone());
        state.claims.insert(b.claim_id.to_string(), b);
        let report = detect_conflicts(&state, "demo");
        assert_eq!(report.conflicts.len(), 1);
    }
}
