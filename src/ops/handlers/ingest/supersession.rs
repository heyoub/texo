use super::super::common::{append_json, take_receipts, workspace_temporal_policy_through};
use super::{
    ExplicitSupersessionHoldReason, ExplicitSupersessionOutcome, HeldExplicitSupersession,
};
use crate::claims::workspace::{ClaimView, WorkspaceView};
use crate::error::TexoError;
use crate::events::coordinate::entity_for_claim;
use crate::events::ids::ClaimId;
use crate::events::machines::{transition_record, TransitionCauseV1, CLAIM_MACHINE};
use crate::events::payloads::{ClaimRecordedV2, ClaimSupersededV2};
use crate::knowledge::TemporalRelation;
use crate::ops::env::ReceiptNote;
use crate::semantics::pipeline::RelateTemporalPolicy;
use batpak::event::EventPayload;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
struct ExplicitReplacementCandidate {
    claim_id: ClaimId,
    workspace_id: String,
    source_id: String,
    normalized_text: String,
    subject_hint: Option<String>,
}

pub(crate) fn infer_supersessions(
    view: &WorkspaceView,
    new_claims: &[ClaimRecordedV2],
    observed_at_ms: u64,
    temporal: &RelateTemporalPolicy,
) -> Result<ExplicitSupersessionOutcome, TexoError> {
    let candidates = new_claims
        .iter()
        .map(|claim| {
            Ok(ExplicitReplacementCandidate {
                claim_id: ClaimId::try_from(claim.claim_id.as_str())?,
                workspace_id: claim.workspace_id.clone(),
                source_id: claim.source_id.clone(),
                normalized_text: claim.normalized_text.clone(),
                subject_hint: claim.subject_hint.clone(),
            })
        })
        .collect::<Result<Vec<_>, TexoError>>()?;
    Ok(infer_explicit_supersessions(
        view,
        &candidates,
        observed_at_ms,
        temporal,
    ))
}

fn infer_indexed_supersessions(
    view: &WorkspaceView,
    new_claim_ids: &BTreeSet<ClaimId>,
    observed_at_ms: u64,
    temporal: &RelateTemporalPolicy,
) -> ExplicitSupersessionOutcome {
    let candidates = view
        .claims
        .iter()
        .filter_map(|claim| {
            let claim_id = ClaimId::try_from(claim.card.claim_id.as_str()).ok()?;
            new_claim_ids
                .contains(&claim_id)
                .then(|| ExplicitReplacementCandidate {
                    claim_id,
                    workspace_id: claim.card.workspace_id.clone(),
                    source_id: claim.card.source_id.clone(),
                    normalized_text: claim.card.normalized_text.clone(),
                    subject_hint: claim.card.subject_hint.clone(),
                })
        })
        .collect::<Vec<_>>();
    infer_explicit_supersessions(view, &candidates, observed_at_ms, temporal)
}

pub(in crate::ops::handlers) fn settle_indexed_explicit_supersessions(
    cx: &mut syncbat::Ctx<'_>,
    view: &WorkspaceView,
    new_claim_ids: &BTreeSet<ClaimId>,
    observed_at_ms: u64,
    receipts: &mut Vec<ReceiptNote>,
) -> Result<ExplicitSupersessionOutcome, TexoError> {
    let evidence_frontier = receipts
        .iter()
        .map(|receipt| receipt.global_sequence)
        .max()
        .unwrap_or(view.frontier);
    let temporal = workspace_temporal_policy_through(view, evidence_frontier)?;
    let outcome = infer_indexed_supersessions(view, new_claim_ids, observed_at_ms, &temporal);
    for supersession in &outcome.applied {
        append_json(
            "texo.knowledge.index",
            cx,
            <ClaimSupersededV2 as EventPayload>::KIND,
            supersession,
        )?;
    }
    receipts.extend(take_receipts()?);
    Ok(outcome)
}

fn infer_explicit_supersessions(
    view: &WorkspaceView,
    new_claims: &[ExplicitReplacementCandidate],
    observed_at_ms: u64,
    temporal: &RelateTemporalPolicy,
) -> ExplicitSupersessionOutcome {
    let mut by_subject: BTreeMap<Option<String>, Vec<&ClaimView>> = BTreeMap::new();
    for claim in &view.claims {
        by_subject
            .entry(claim.card.subject_hint.clone())
            .or_default()
            .push(claim);
    }
    let mut applied = Vec::new();
    let mut held = Vec::new();
    let mut seen_old = BTreeSet::new();
    for new_claim in new_claims {
        if !replacement_signal(&new_claim.normalized_text) {
            continue;
        }
        let Some(candidates) = by_subject.get(&new_claim.subject_hint) else {
            continue;
        };
        for old in candidates {
            if old.card.claim_id == new_claim.claim_id.as_str()
                || old.card.phase != 1
                || old.card.normalized_text == new_claim.normalized_text
            {
                continue;
            }
            let Ok(old_claim_id) = ClaimId::try_from(old.card.claim_id.as_str()) else {
                continue;
            };
            let hold_reason = match temporal.compare_claims(&old_claim_id, &new_claim.claim_id) {
                None | Some(TemporalRelation::Same | TemporalRelation::Before) => None,
                Some(TemporalRelation::After) => {
                    Some(ExplicitSupersessionHoldReason::TemporalReversed)
                }
                Some(TemporalRelation::Concurrent) => {
                    Some(ExplicitSupersessionHoldReason::TemporalConcurrent)
                }
                Some(TemporalRelation::Unknown) => {
                    Some(ExplicitSupersessionHoldReason::TemporalUnknown)
                }
            };
            if !seen_old.insert(old.card.claim_id.clone()) {
                continue;
            }
            if let Some(reason) = hold_reason {
                held.push(HeldExplicitSupersession {
                    old_claim_id,
                    new_claim_id: new_claim.claim_id.clone(),
                    reason,
                });
                continue;
            }
            let old_entity = entity_for_claim(&old.card.claim_id);
            applied.push(ClaimSupersededV2 {
                old_claim_id: old.card.claim_id.clone(),
                new_claim_id: new_claim.claim_id.to_string(),
                workspace_id: new_claim.workspace_id.clone(),
                reason: "explicit replacement wording accepted by temporal policy".to_string(),
                decided_by: "texo.ingest.run".to_string(),
                observed_at_ms,
                transition: transition_record(
                    CLAIM_MACHINE,
                    &old_entity,
                    1,
                    2,
                    vec![TransitionCauseV1 {
                        lane: 0,
                        key: format!("ingest:{}", new_claim.source_id),
                    }],
                    observed_at_ms,
                ),
            });
        }
    }
    applied.sort_by(|left, right| {
        left.old_claim_id
            .cmp(&right.old_claim_id)
            .then_with(|| left.new_claim_id.cmp(&right.new_claim_id))
    });
    held.sort_by(|left, right| {
        left.old_claim_id
            .cmp(&right.old_claim_id)
            .then_with(|| left.new_claim_id.cmp(&right.new_claim_id))
    });
    ExplicitSupersessionOutcome { applied, held }
}

fn replacement_signal(normalized_text: &str) -> bool {
    crate::lexicon::contains_replacement_signal(normalized_text)
}
