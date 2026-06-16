//! Apply texo events to claim state.

use crate::events::envelope::TexoEvent;
use crate::events::payloads::{
    ClaimConflictDetected, ClaimRecorded, ClaimSuperseded, SourceObserved,
};
use crate::replay::state::{ClaimState, ClaimView, ConflictView, SourceView, SupersessionView};
use crate::state::claim_lifecycle::{
    apply_open_conflict, initial_claim_status, transition, validate_supersession,
    ClaimLifecycleEvent,
};
use crate::state::TransitionError;
use crate::types::ids::{ClaimId, ConflictId, SourceId};
use crate::types::status::{ConflictStatus, ConflictStatusParseError};
use crate::types::IdParseError;

/// Apply one journal event to replay state.
pub fn apply_event(state: &mut ClaimState, event: &TexoEvent) -> Result<(), ReplayError> {
    state.note_sequence(event.sequence());
    match event {
        TexoEvent::SourceObserved { payload, receipt } => apply_source(state, payload, receipt),
        TexoEvent::ClaimRecorded { payload, receipt } => apply_claim(state, payload, receipt),
        TexoEvent::ClaimSuperseded { payload, receipt } => {
            apply_supersession_event(state, payload, receipt)
        }
        TexoEvent::ClaimConflictDetected { payload, receipt } => {
            apply_conflict(state, payload, receipt)
        }
        TexoEvent::OnboardingCompiled { .. } => Ok(()),
    }
}

fn apply_source(
    state: &mut ClaimState,
    payload: &SourceObserved,
    receipt: &crate::types::receipt::ReceiptView,
) -> Result<(), ReplayError> {
    let source_id = SourceId::try_from(payload.source_id.as_str())?;
    state.sources.insert(
        source_id.clone(),
        SourceView {
            source_id,
            workspace_id: payload.workspace_id.clone(),
            source_kind: payload.source_kind.clone(),
            path: payload.path.clone(),
            body_hash_hex: payload.body_hash_hex.clone(),
            observed_at_ms: payload.observed_at_ms,
            receipt: receipt.clone(),
        },
    );
    Ok(())
}

fn apply_claim(
    state: &mut ClaimState,
    payload: &ClaimRecorded,
    receipt: &crate::types::receipt::ReceiptView,
) -> Result<(), ReplayError> {
    let claim_id = ClaimId::try_from(payload.claim_id.as_str())?;
    let source_id = SourceId::try_from(payload.source_id.as_str())?;
    let view = ClaimView {
        claim_id: claim_id.clone(),
        workspace_id: payload.workspace_id.clone(),
        source_id,
        source_path: payload.source_path.clone(),
        line_start: payload.line_start,
        line_end: payload.line_end,
        text: payload.text.clone(),
        normalized_text: payload.normalized_text.clone(),
        subject_hint: payload.subject_hint.clone(),
        predicate_hint: payload.predicate_hint.clone(),
        object_hint: payload.object_hint.clone(),
        confidence_ppm: payload.confidence_ppm,
        extractor_kind: payload.extractor_kind.clone(),
        // `Recorded` is total: a newly recorded claim is always `Current`.
        status: initial_claim_status(),
        receipt: receipt.clone(),
        supersedes: Vec::new(),
        superseded_by: None,
    };
    state.claims.insert(claim_id, view);
    state.rebuild_subject_index();
    Ok(())
}

fn apply_supersession_event(
    state: &mut ClaimState,
    payload: &ClaimSuperseded,
    receipt: &crate::types::receipt::ReceiptView,
) -> Result<(), ReplayError> {
    let old_id = ClaimId::try_from(payload.old_claim_id.as_str())?;
    let new_id = ClaimId::try_from(payload.new_claim_id.as_str())?;

    validate_supersession(payload)?;

    if !state.claims.contains_key(&old_id) {
        return Err(ReplayError::MissingClaim(old_id));
    }
    if !state.claims.contains_key(&new_id) {
        return Err(ReplayError::MissingClaim(new_id));
    }

    if let Some(old) = state.claims.get_mut(&old_id) {
        old.status = transition(old.status, ClaimLifecycleEvent::Superseded)?;
        old.superseded_by = Some(new_id.clone());
    }

    if let Some(new) = state.claims.get_mut(&new_id) {
        new.supersedes.push(old_id.clone());
    }

    state.superseded.insert(
        old_id.clone(),
        SupersessionView {
            old_claim_id: old_id,
            new_claim_id: new_id,
            reason: payload.reason.clone(),
            decided_by: payload.decided_by.clone(),
            receipt: receipt.clone(),
        },
    );
    state.rebuild_subject_index();
    Ok(())
}

fn apply_conflict(
    state: &mut ClaimState,
    payload: &ClaimConflictDetected,
    receipt: &crate::types::receipt::ReceiptView,
) -> Result<(), ReplayError> {
    let conflict_id = ConflictId::try_from(payload.conflict_id.as_str())?;
    let claim_a = ClaimId::try_from(payload.claim_a.as_str())?;
    let claim_b = ClaimId::try_from(payload.claim_b.as_str())?;
    let status = ConflictStatus::parse_str(&payload.status)?;

    if status == ConflictStatus::Open {
        for id in [&claim_a, &claim_b] {
            if let Some(claim) = state.claims.get_mut(id) {
                // `OpenConflict` is total; call the total function directly so no
                // error can be swallowed and a stale status silently kept.
                claim.status = apply_open_conflict(claim.status);
            }
        }
    }

    state.conflicts.insert(
        conflict_id.clone(),
        ConflictView {
            conflict_id,
            claim_a,
            claim_b,
            reason: payload.reason.clone(),
            status,
            receipt: receipt.clone(),
        },
    );
    state.rebuild_subject_index();
    Ok(())
}

/// Replay errors.
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    /// Invalid identifier in event payload.
    #[error("invalid id: {0}")]
    InvalidId(#[from] IdParseError),
    /// Illegal lifecycle transition.
    #[error("transition: {0}")]
    Transition(#[from] TransitionError),
    /// Referenced claim is absent from replay state.
    #[error("missing claim: {0}")]
    MissingClaim(ClaimId),
    /// Unrecognized conflict status string in event payload.
    #[error("invalid conflict status: {0}")]
    InvalidStatus(#[from] ConflictStatusParseError),
}

#[cfg(test)]
mod tests {
    //! PROVES: INV-REPLAY-ERRORS (F3/F4 error paths) — folding domain events
    //! directly through the reducer must surface MissingClaim, Transition
    //! (self-supersession) and InvalidStatus rather than panicking or silently
    //! succeeding.
    use super::*;
    use crate::events::payloads::{ClaimConflictDetected, ClaimRecorded, ClaimSuperseded};
    use crate::replay::reducer::fold_events;
    use crate::types::receipt::receipt_view;

    const SOURCE_ID: &str = "src_abc123def456";

    fn recorded_event(claim_id: &str, sequence: u64) -> TexoEvent {
        TexoEvent::ClaimRecorded {
            payload: ClaimRecorded {
                claim_id: claim_id.to_string(),
                workspace_id: "demo".to_string(),
                source_id: SOURCE_ID.to_string(),
                source_path: "x.md".to_string(),
                line_start: 1,
                line_end: 1,
                text: "x".to_string(),
                normalized_text: "x".to_string(),
                subject_hint: "s".to_string(),
                predicate_hint: "unknown".to_string(),
                object_hint: "x".to_string(),
                confidence_ppm: 500_000,
                extractor_kind: "test".to_string(),
                observed_at_ms: 1,
            },
            receipt: receipt_view(sequence.into(), sequence, "ClaimRecorded", "demo", claim_id),
        }
    }

    fn supersede_event(old: &str, new: &str, sequence: u64) -> TexoEvent {
        TexoEvent::ClaimSuperseded {
            payload: ClaimSuperseded {
                old_claim_id: old.to_string(),
                new_claim_id: new.to_string(),
                workspace_id: "demo".to_string(),
                reason: "test".to_string(),
                decided_by: "test".to_string(),
                observed_at_ms: 2,
            },
            receipt: receipt_view(sequence.into(), sequence, "ClaimSuperseded", "demo", old),
        }
    }

    fn conflict_event(status: &str, claim_a: &str, claim_b: &str, sequence: u64) -> TexoEvent {
        TexoEvent::ClaimConflictDetected {
            payload: ClaimConflictDetected {
                conflict_id: "conflict_aaaaaaaaaaaa".to_string(),
                workspace_id: "demo".to_string(),
                claim_a: claim_a.to_string(),
                claim_b: claim_b.to_string(),
                reason: "test".to_string(),
                status: status.to_string(),
                observed_at_ms: 3,
            },
            receipt: receipt_view(
                sequence.into(),
                sequence,
                "ClaimConflictDetected",
                "demo",
                "conflict_aaaaaaaaaaaa",
            ),
        }
    }

    #[test]
    fn supersession_missing_old_claim_errors() {
        // new_claim recorded but old_claim never recorded.
        let new_id = "claim_bbbbbbbbbbbb";
        let old_id = "claim_aaaaaaaaaaaa";
        let events = [
            recorded_event(new_id, 1),
            supersede_event(old_id, new_id, 2),
        ];
        let result = fold_events(&events);
        match result {
            Err(ReplayError::MissingClaim(id)) => assert_eq!(id.as_str(), old_id),
            other => panic!("expected MissingClaim({old_id}), got {other:?}"),
        }
    }

    #[test]
    fn supersession_missing_new_claim_errors() {
        // old_claim recorded but new_claim never recorded.
        let old_id = "claim_aaaaaaaaaaaa";
        let new_id = "claim_bbbbbbbbbbbb";
        let events = [
            recorded_event(old_id, 1),
            supersede_event(old_id, new_id, 2),
        ];
        let result = fold_events(&events);
        match result {
            Err(ReplayError::MissingClaim(id)) => assert_eq!(id.as_str(), new_id),
            other => panic!("expected MissingClaim({new_id}), got {other:?}"),
        }
    }

    #[test]
    fn self_supersession_errors_with_transition() {
        // old_claim_id == new_claim_id must be rejected by validate_supersession.
        let id = "claim_aaaaaaaaaaaa";
        let events = [recorded_event(id, 1), supersede_event(id, id, 2)];
        let result = fold_events(&events);
        assert!(
            matches!(result, Err(ReplayError::Transition(_))),
            "self-supersession must produce ReplayError::Transition, got {result:?}"
        );
    }

    #[test]
    fn conflict_with_unparsable_status_errors() {
        let a = "claim_aaaaaaaaaaaa";
        let b = "claim_bbbbbbbbbbbb";
        let events = [
            recorded_event(a, 1),
            recorded_event(b, 2),
            conflict_event("not_a_status", a, b, 3),
        ];
        let result = fold_events(&events);
        match result {
            Err(ReplayError::InvalidStatus(s)) => assert_eq!(s.value, "not_a_status"),
            other => panic!("expected InvalidStatus(not_a_status), got {other:?}"),
        }
    }
}
