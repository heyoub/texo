//! Read-only conflict detection.

use crate::extract::word_match::contains_word;
use crate::replay::state::{ClaimState, ClaimView};
use crate::stale::check::has_replacement_keyword;
use crate::state::conflict_lifecycle::{ConflictEntry, ConflictReport};
use crate::types::ids::{conflict_id_from_pair, ClaimId, WorkspaceId};
use crate::types::status::{ClaimStatus, ConflictStatus};

const CONTRADICTION_SUBJECTS: &[&str] = &[
    "deploy-process",
    "release-process",
    "approval-process",
    "ownership",
];

/// Detect conflicts from current replayed state (read-only).
pub fn detect_conflicts(state: &ClaimState, workspace_id: &WorkspaceId) -> ConflictReport {
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
        workspace_id: workspace_id.clone(),
        conflicts,
    }
}

fn already_superseded_pair(state: &ClaimState, a: &ClaimId, b: &ClaimId) -> bool {
    state.superseded.contains_key(a) || state.superseded.contains_key(b)
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
    let a_neg = contains_word(&a.normalized_text, "not");
    let b_neg = contains_word(&b.normalized_text, "not");
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
    let a_day = SCHEDULE_DAYS
        .iter()
        .find(|d| contains_word(&a.object_hint, d));
    let b_day = SCHEDULE_DAYS
        .iter()
        .find(|d| contains_word(&b.object_hint, d));
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
        if !state.sources.contains_key(&claim.source_id) {
            return Err(VerifyError::Projection(format!(
                "claim {} references unknown source {}",
                claim.claim_id, claim.source_id
            )));
        }
    }
    for sup in state.superseded.values() {
        if !state.claims.contains_key(&sup.old_claim_id) {
            return Err(VerifyError::Projection(format!(
                "supersession references unknown old claim {}",
                sup.old_claim_id
            )));
        }
        if !state.claims.contains_key(&sup.new_claim_id) {
            return Err(VerifyError::Projection(format!(
                "supersession references unknown new claim {}",
                sup.new_claim_id
            )));
        }
    }
    Ok(())
}

/// Verification errors for projection and journal receipts.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// Projection invariant violation.
    #[error("projection: {0}")]
    Projection(String),
    /// Journal event decode failure during receipt verification.
    #[error("journal: {0}")]
    Decode(#[from] crate::events::envelope::DecodeError),
    /// Journal receipt re-verification failure.
    #[error("journal: {0}")]
    Journal(#[from] crate::journal::JournalError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::state::ClaimView;
    use crate::types::ids::SourceId;
    use crate::types::receipt::receipt_view;
    use assert_matches::assert_matches;

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
        state.claims.insert(a.claim_id.clone(), a.clone());
        state.claims.insert(b.claim_id.clone(), b);
        let workspace = WorkspaceId::new("demo").expect("workspace");
        let report = detect_conflicts(&state, &workspace);
        assert_eq!(report.conflicts.len(), 1);
        let entry = &report.conflicts[0];
        // The conflict must name exactly the two contradictory claims (order
        // independent) on the deploy-process subject, flagged Open.
        let mut pair = [entry.claim_a.as_str(), entry.claim_b.as_str()];
        pair.sort_unstable();
        assert_eq!(pair, ["claim_aaaaaaaaaaaa", "claim_bbbbbbbbbbbb"]);
        assert_eq!(entry.subject_hint, "deploy-process");
        assert_eq!(entry.status, ConflictStatus::Open);
        assert!(entry.reason.contains("contradictory") && entry.reason.contains("deploy-process"));
        // The conflict id must be the deterministic pair id (stable regardless of
        // detection order) so commit/dedup downstream is reproducible.
        assert_eq!(
            entry.conflict_id,
            conflict_id_from_pair(&entry.claim_a, &entry.claim_b)
        );
    }

    fn claim_conf(
        id: &str,
        subject: &str,
        text: &str,
        predicate: &str,
        object: &str,
        confidence_ppm: u32,
    ) -> ClaimView {
        let mut c = claim(id, subject, text, predicate, object);
        c.confidence_ppm = confidence_ppm;
        c
    }

    fn report_for(a: ClaimView, b: ClaimView) -> ConflictReport {
        let mut state = ClaimState::default();
        state.claims.insert(a.claim_id.clone(), a);
        state.claims.insert(b.claim_id.clone(), b);
        let workspace = WorkspaceId::new("demo").expect("workspace");
        detect_conflicts(&state, &workspace)
    }

    #[test]
    fn replacement_keyword_in_text_flags_conflict() {
        // The replacement-keyword path (has_replacement_keyword over a.text)
        // must fire even when subjects are an arbitrary non-listed slug and no
        // other signal is present.
        let a = claim(
            "claim_aaaaaaaaaaaa",
            "batch-jobs",
            "Batch jobs run nightly.",
            "unknown",
            "nightly",
        );
        let b = claim(
            "claim_bbbbbbbbbbbb",
            "batch-jobs",
            "Batch jobs moved to hourly.",
            "unknown",
            "hourly",
        );
        let report = report_for(a, b);
        assert_eq!(report.conflicts.len(), 1, "replacement keyword must flag");
        assert_eq!(report.conflicts[0].subject_hint, "batch-jobs");
    }

    #[test]
    fn predicate_changed_flags_conflict() {
        // One side carries the "changed" predicate while the other does not:
        // the predicate-change path must flag without any keyword/negation.
        let a = claim(
            "claim_aaaaaaaaaaaa",
            "batch-jobs",
            "Batch jobs run nightly.",
            "is",
            "nightly",
        );
        let b = claim(
            "claim_bbbbbbbbbbbb",
            "batch-jobs",
            "Batch jobs hourly window.",
            "changed",
            "hourly",
        );
        let report = report_for(a, b);
        assert_eq!(report.conflicts.len(), 1, "predicate change must flag");
        let entry = &report.conflicts[0];
        let pair = [entry.claim_a.as_str(), entry.claim_b.as_str()];
        assert!(pair.contains(&"claim_aaaaaaaaaaaa") && pair.contains(&"claim_bbbbbbbbbbbb"));
    }

    #[test]
    fn owns_with_differing_objects_flags_conflict() {
        // Two ownership claims on the same subject pointing at different owners
        // must conflict via the owns/owns object-mismatch path.
        let a = claim(
            "claim_aaaaaaaaaaaa",
            "ownership",
            "Alice owns billing.",
            "owns",
            "alice",
        );
        let b = claim(
            "claim_bbbbbbbbbbbb",
            "ownership",
            "Bob owns billing.",
            "owns",
            "bob",
        );
        let report = report_for(a, b);
        assert_eq!(report.conflicts.len(), 1, "owns object clash must flag");
        assert_eq!(report.conflicts[0].subject_hint, "ownership");
    }

    #[test]
    fn confidence_gap_flags_conflict() {
        // A large confidence gap (>250000ppm) between two differing claims on a
        // non-listed subject must trip the confidence_gap path even though no
        // keyword, negation, predicate-change, or schedule signal is present.
        let a = claim_conf(
            "claim_aaaaaaaaaaaa",
            "freeform",
            "The widget ships green.",
            "is",
            "green",
            900_000,
        );
        let b = claim_conf(
            "claim_bbbbbbbbbbbb",
            "freeform",
            "The widget ships red.",
            "is",
            "red",
            500_000,
        );
        let report = report_for(a, b);
        assert_eq!(report.conflicts.len(), 1, "confidence gap must flag");
    }

    #[test]
    fn contradiction_subject_with_object_clash_flags_conflict() {
        // On a listed CONTRADICTION_SUBJECTS subject (approval-process), two
        // non-empty differing objects must conflict via the final subject-list
        // path — with predicates equal and no keyword/negation present.
        let a = claim(
            "claim_aaaaaaaaaaaa",
            "approval-process",
            "Approval handled by team x.",
            "is",
            "team-x",
        );
        let b = claim(
            "claim_bbbbbbbbbbbb",
            "approval-process",
            "Approval handled by team y.",
            "is",
            "team-y",
        );
        let report = report_for(a, b);
        assert_eq!(report.conflicts.len(), 1, "subject-list object clash flags");
        assert_eq!(report.conflicts[0].subject_hint, "approval-process");
    }

    #[test]
    fn no_signal_pair_produces_no_conflict() {
        // Same subject, equal predicate and equal object, no keyword/negation,
        // small confidence gap, and the subject is not contradiction-listed:
        // none of the signals fire, so no conflict is reported.
        let a = claim(
            "claim_aaaaaaaaaaaa",
            "freeform",
            "The widget ships green.",
            "is",
            "green",
        );
        let b = claim(
            "claim_bbbbbbbbbbbb",
            "freeform",
            "The gadget ships green.",
            "is",
            "green",
        );
        let report = report_for(a, b);
        // No signal fires, so no conflict is reported.
        assert_eq!(report.conflicts, Vec::new());
    }

    #[test]
    fn superseded_pair_is_skipped() {
        // Even a clear negation contradiction must NOT be reported once one of
        // the claims is already superseded (already_superseded_pair short-circuit).
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
        state.claims.insert(a.claim_id.clone(), a.clone());
        state.claims.insert(b.claim_id.clone(), b.clone());
        state.superseded.insert(
            a.claim_id.clone(),
            crate::replay::state::SupersessionView {
                old_claim_id: a.claim_id.clone(),
                new_claim_id: b.claim_id.clone(),
                reason: "test".to_string(),
                decided_by: "test".to_string(),
                receipt: receipt_view(
                    1,
                    1,
                    "ClaimSuperseded",
                    "workspace:demo",
                    a.claim_id.as_str(),
                ),
            },
        );
        let workspace = WorkspaceId::new("demo").expect("workspace");
        let report = detect_conflicts(&state, &workspace);
        // An already-superseded pair must be skipped, even with a clear signal.
        assert_eq!(report.conflicts, Vec::new());
    }

    #[test]
    fn equal_normalized_text_is_skipped() {
        // Two claims whose normalized text is identical are duplicates, not
        // contradictions, and must be skipped before any signal check.
        let a = claim(
            "claim_aaaaaaaaaaaa",
            "deploy-process",
            "Deploys moved to Friday.",
            "changed",
            "friday",
        );
        let mut b = a.clone();
        b.claim_id = ClaimId::try_from("claim_bbbbbbbbbbbb").expect("id");
        let report = report_for(a, b);
        // Identical normalized text is a duplicate, not a contradiction.
        assert_eq!(report.conflicts, Vec::new());
    }

    #[test]
    fn schedule_clash_only_on_deploy_or_release_subjects() {
        // Friday-vs-Tuesday objects on a NON schedule subject must not trip the
        // schedule_object_clash path (guarded to deploy/release subjects).
        assert!(!schedule_object_clash(
            &claim("claim_aaaaaaaaaaaa", "ownership", "x", "owns", "friday"),
            &claim("claim_bbbbbbbbbbbb", "ownership", "y", "owns", "tuesday"),
            "ownership"
        ));
        // Same days on a deploy subject DO clash.
        assert!(schedule_object_clash(
            &claim("claim_aaaaaaaaaaaa", "release-process", "x", "is", "friday"),
            &claim(
                "claim_bbbbbbbbbbbb",
                "release-process",
                "y",
                "is",
                "tuesday"
            ),
            "release-process"
        ));
        // Identical days do not clash.
        assert!(!schedule_object_clash(
            &claim("claim_aaaaaaaaaaaa", "deploy-process", "x", "is", "friday"),
            &claim("claim_bbbbbbbbbbbb", "deploy-process", "y", "is", "friday"),
            "deploy-process"
        ));
    }

    #[test]
    fn negation_mismatch_requires_same_stripped_text() {
        // Both-negated (a_neg == b_neg) returns false: not a mismatch.
        assert!(!negation_mismatch(
            &claim(
                "claim_aaaaaaaaaaaa",
                "s",
                "Deploys are not allowed.",
                "is",
                "x"
            ),
            &claim(
                "claim_bbbbbbbbbbbb",
                "s",
                "Releases are not allowed.",
                "is",
                "x"
            ),
        ));
        // One negated, one not, but the stripped texts differ -> false.
        assert!(!negation_mismatch(
            &claim("claim_aaaaaaaaaaaa", "s", "Deploys are allowed.", "is", "x"),
            &claim(
                "claim_bbbbbbbbbbbb",
                "s",
                "Releases are not allowed.",
                "is",
                "x"
            ),
        ));
    }

    #[test]
    fn verify_projection_accepts_consistent_state() {
        // Empty state has a zero frontier and no claims: the projection holds.
        let state = ClaimState::default();
        verify_projection(&state).expect("empty state is consistent");
    }

    #[test]
    fn verify_projection_rejects_zero_frontier_with_claims() {
        let a = claim("claim_aaaaaaaaaaaa", "s", "x", "is", "x");
        let mut state = ClaimState::default();
        state.claims.insert(a.claim_id.clone(), a);
        // replayed_through_sequence stays 0 while a claim exists -> invariant fail.
        let err = verify_projection(&state).expect_err("zero frontier must fail");
        assert_matches!(err, VerifyError::Projection(msg) if msg.contains("frontier"));
    }

    #[test]
    fn verify_projection_rejects_claim_with_unknown_source() {
        let a = claim("claim_aaaaaaaaaaaa", "s", "x", "is", "x");
        let mut state = ClaimState {
            replayed_through_sequence: 1,
            ..Default::default()
        };
        state.claims.insert(a.claim_id.clone(), a);
        // No matching source inserted -> dangling source reference.
        let err = verify_projection(&state).expect_err("unknown source must fail");
        assert_matches!(err, VerifyError::Projection(msg) if msg.contains("unknown source"));
    }

    #[test]
    fn verify_projection_rejects_supersession_with_unknown_claims() {
        use crate::replay::state::{SourceView, SupersessionView};
        use crate::types::ids::SourceId;
        let a = claim("claim_aaaaaaaaaaaa", "s", "x", "is", "x");
        let source_id = SourceId::try_from("src_abc123def456").expect("src");
        let mut state = ClaimState {
            replayed_through_sequence: 1,
            ..Default::default()
        };
        state.sources.insert(
            source_id.clone(),
            SourceView {
                source_id: source_id.clone(),
                workspace_id: "demo".to_string(),
                source_kind: "markdown".to_string(),
                path: "x.md".to_string(),
                body_hash_hex: "00".repeat(32),
                observed_at_ms: 1,
                receipt: receipt_view(1, 1, "SourceObserved", "workspace:demo", "src_abc123def456"),
            },
        );
        state.claims.insert(a.claim_id.clone(), a.clone());
        // Supersession edge references an old claim that is not in state.claims.
        let ghost = ClaimId::try_from("claim_cccccccccccc").expect("id");
        state.superseded.insert(
            ghost.clone(),
            SupersessionView {
                old_claim_id: ghost.clone(),
                new_claim_id: a.claim_id.clone(),
                reason: "test".to_string(),
                decided_by: "test".to_string(),
                receipt: receipt_view(2, 2, "ClaimSuperseded", "workspace:demo", ghost.as_str()),
            },
        );
        let err = verify_projection(&state).expect_err("unknown old claim must fail");
        assert_matches!(err, VerifyError::Projection(msg) if msg.contains("unknown old claim"));

        // Now make the OLD reference valid but the NEW reference dangle.
        state.superseded.clear();
        let ghost_new = ClaimId::try_from("claim_dddddddddddd").expect("id");
        state.superseded.insert(
            a.claim_id.clone(),
            SupersessionView {
                old_claim_id: a.claim_id.clone(),
                new_claim_id: ghost_new.clone(),
                reason: "test".to_string(),
                decided_by: "test".to_string(),
                receipt: receipt_view(
                    3,
                    3,
                    "ClaimSuperseded",
                    "workspace:demo",
                    a.claim_id.as_str(),
                ),
            },
        );
        let err = verify_projection(&state).expect_err("unknown new claim must fail");
        assert_matches!(err, VerifyError::Projection(msg) if msg.contains("unknown new claim"));
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
        state.claims.insert(a.claim_id.clone(), a.clone());
        state.claims.insert(b.claim_id.clone(), b);
        let workspace = WorkspaceId::new("demo").expect("workspace");
        let report = detect_conflicts(&state, &workspace);
        assert_eq!(report.conflicts.len(), 1);
        let entry = &report.conflicts[0];
        // Friday-vs-Tuesday must be flagged on the deploy-process subject even
        // with no replacement keyword present (pure schedule-object clash path).
        let mut pair = [entry.claim_a.as_str(), entry.claim_b.as_str()];
        pair.sort_unstable();
        assert_eq!(pair, ["claim_aaaaaaaaaaaa", "claim_bbbbbbbbbbbb"]);
        assert_eq!(entry.subject_hint, "deploy-process");
        assert_eq!(entry.status, ConflictStatus::Open);
    }
}
