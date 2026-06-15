//! Apply texo events to claim state.

use crate::events::envelope::TexoEvent;
use crate::events::payloads::{
    ClaimConflictDetected, ClaimRecorded, ClaimSuperseded, SourceObserved,
};
use crate::replay::state::{ClaimState, ClaimView, ConflictView, SourceView, SupersessionView};
use crate::state::claim_lifecycle::{initial_claim_status, transition, ClaimLifecycleEvent};
use crate::types::ids::{ClaimId, ConflictId, SourceId};
use crate::types::status::{ClaimStatus, ConflictStatus};
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
    let source_id = SourceId::try_from(payload.source_id.as_str())
        .map_err(|e: IdParseError| ReplayError::InvalidId(e.to_string()))?;
    state.sources.insert(
        payload.source_id.clone(),
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
    let claim_id = ClaimId::try_from(payload.claim_id.as_str())
        .map_err(|e: IdParseError| ReplayError::InvalidId(e.to_string()))?;
    let source_id = SourceId::try_from(payload.source_id.as_str())
        .map_err(|e: IdParseError| ReplayError::InvalidId(e.to_string()))?;
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
        status: transition(ClaimStatus::Unknown, ClaimLifecycleEvent::Recorded)
            .unwrap_or(initial_claim_status()),
        receipt: receipt.clone(),
        supersedes: Vec::new(),
        superseded_by: None,
    };
    state.claims.insert(payload.claim_id.clone(), view);
    state.rebuild_subject_index();
    Ok(())
}

fn apply_supersession_event(
    state: &mut ClaimState,
    payload: &ClaimSuperseded,
    receipt: &crate::types::receipt::ReceiptView,
) -> Result<(), ReplayError> {
    let old_id = ClaimId::try_from(payload.old_claim_id.as_str())
        .map_err(|e: IdParseError| ReplayError::InvalidId(e.to_string()))?;
    let new_id = ClaimId::try_from(payload.new_claim_id.as_str())
        .map_err(|e: IdParseError| ReplayError::InvalidId(e.to_string()))?;

    if let Some(old) = state.claims.get_mut(&payload.old_claim_id) {
        old.status = transition(old.status, ClaimLifecycleEvent::Superseded)
            .map_err(|e| ReplayError::Transition(e.to_string()))?;
        old.superseded_by = Some(new_id.clone());
    }

    if let Some(new) = state.claims.get_mut(&payload.new_claim_id) {
        new.supersedes.push(old_id.clone());
    }

    state.superseded.insert(
        payload.old_claim_id.clone(),
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
    let conflict_id = ConflictId::try_from(payload.conflict_id.as_str())
        .map_err(|e: IdParseError| ReplayError::InvalidId(e.to_string()))?;
    let claim_a = ClaimId::try_from(payload.claim_a.as_str())
        .map_err(|e: IdParseError| ReplayError::InvalidId(e.to_string()))?;
    let claim_b = ClaimId::try_from(payload.claim_b.as_str())
        .map_err(|e: IdParseError| ReplayError::InvalidId(e.to_string()))?;
    let status = ConflictStatus::parse_str(&payload.status).unwrap_or(ConflictStatus::Open);

    if status == ConflictStatus::Open {
        for id in [&payload.claim_a, &payload.claim_b] {
            if let Some(claim) = state.claims.get_mut(id.as_str()) {
                claim.status = transition(claim.status, ClaimLifecycleEvent::OpenConflict)
                    .unwrap_or(claim.status);
            }
        }
    }

    state.conflicts.insert(
        payload.conflict_id.clone(),
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
    InvalidId(String),
    /// Illegal lifecycle transition.
    #[error("transition: {0}")]
    Transition(String),
}
