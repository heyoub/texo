use super::super::common::append_json;
use super::backend::SemanticRelateOutput;
use super::{RelateConflictRow, RelateSupersessionRow};
use crate::error::TexoError;
use crate::events::coordinate::{entity_for_claim, entity_for_conflict};
use crate::events::ids::WorkspaceId;
use crate::events::machines::{
    transition_record, TransitionCauseV1, CLAIM_MACHINE, CONFLICT_MACHINE,
};
use crate::events::payloads::{
    ClaimSupersededV2, ConflictOpenedV2, RelationDeferredV1, RelationJudgedV1,
};
use crate::semantics::pipeline::RelateOutcome;
use crate::semantics::score::unit_interval_to_ppm;
use batpak::event::EventPayload;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn append_relation_judgments(
    op: &'static str,
    cx: &mut syncbat::Ctx<'_>,
    workspace_id: &WorkspaceId,
    related: &SemanticRelateOutput,
    observed_at_ms: u64,
) -> Result<(), TexoError> {
    for judgment in related.outcome.judgments() {
        if judgment.reused_authority {
            continue;
        }
        let cache_key_hex = related
            .cache_keys
            .get(&(
                judgment.older_claim.to_string(),
                judgment.newer_claim.to_string(),
            ))
            .cloned()
            .unwrap_or_default();
        append_json(
            op,
            cx,
            <RelationJudgedV1 as EventPayload>::KIND,
            &RelationJudgedV1 {
                workspace_id: workspace_id.clone(),
                older_claim: judgment.older_claim.clone(),
                newer_claim: judgment.newer_claim.clone(),
                relation: judgment.verdict.relation.into(),
                score_ppm: unit_interval_to_ppm(judgment.verdict.score),
                judge_fingerprint: related.judge_fingerprint.clone(),
                cache_key_hex,
                observed_at_ms,
            },
        )?;
    }
    Ok(())
}

pub(super) fn append_relation_deferrals(
    op: &'static str,
    cx: &mut syncbat::Ctx<'_>,
    workspace_id: &WorkspaceId,
    unresolved: &[crate::relate::settlement::UnresolvedPair],
    observed_at_ms: u64,
) -> Result<(), TexoError> {
    for unresolved in unresolved {
        append_json(
            op,
            cx,
            <RelationDeferredV1 as EventPayload>::KIND,
            &RelationDeferredV1 {
                workspace_id: workspace_id.clone(),
                older_claim: unresolved.old_claim.clone(),
                newer_claim: unresolved.new_claim.clone(),
                failure_class: unresolved.failure.class,
                attempts: unresolved.failure.attempts,
                observed_at_ms,
            },
        )?;
    }
    Ok(())
}

pub(super) struct RelatePublicationPlan {
    pub(super) supersessions: Vec<crate::semantics::pipeline::SupersessionEdge>,
    pub(super) conflicts: Vec<crate::semantics::pipeline::ConflictEntry>,
}

pub(super) fn relate_publication(outcome: &RelateOutcome) -> RelatePublicationPlan {
    match outcome {
        RelateOutcome::Complete(complete) => RelatePublicationPlan {
            supersessions: complete.related.supersessions.clone(),
            conflicts: complete.related.conflicts.clone(),
        },
        RelateOutcome::Partial(_) => RelatePublicationPlan {
            supersessions: Vec::new(),
            conflicts: Vec::new(),
        },
    }
}

pub(super) fn append_relate_supersessions(
    op: &'static str,
    cx: &mut syncbat::Ctx<'_>,
    workspace_id: &str,
    decisions: &[crate::semantics::pipeline::SupersessionEdge],
    cache_keys: &BTreeMap<(String, String), String>,
    observed_at_ms: u64,
) -> Result<Vec<RelateSupersessionRow>, TexoError> {
    let mut rows = Vec::new();
    for (old, new, reason) in decisions {
        let old_id = old.to_string();
        let new_id = new.to_string();
        let cache_key = cache_keys
            .get(&(old_id.clone(), new_id.clone()))
            .cloned()
            .unwrap_or_default();
        append_json(
            op,
            cx,
            <ClaimSupersededV2 as EventPayload>::KIND,
            &ClaimSupersededV2 {
                old_claim_id: old_id.clone(),
                new_claim_id: new_id.clone(),
                workspace_id: workspace_id.to_string(),
                reason: reason.clone(),
                decided_by: "texo-relate".to_string(),
                observed_at_ms,
                transition: transition_record(
                    CLAIM_MACHINE,
                    &entity_for_claim(&old_id),
                    1,
                    2,
                    vec![TransitionCauseV1 {
                        lane: 0,
                        key: format!("relate:{cache_key}"),
                    }],
                    observed_at_ms,
                ),
            },
        )?;
        rows.push(RelateSupersessionRow {
            old_claim_id: old_id,
            new_claim_id: new_id,
            reason: reason.clone(),
            cache_key,
        });
    }
    Ok(rows)
}

pub(super) fn append_relate_conflicts(
    op: &'static str,
    cx: &mut syncbat::Ctx<'_>,
    workspace_id: &str,
    decisions: &[crate::semantics::pipeline::ConflictEntry],
    existing: &BTreeSet<String>,
    cache_keys: &BTreeMap<(String, String), String>,
    observed_at_ms: u64,
) -> Result<Vec<RelateConflictRow>, TexoError> {
    let mut rows = Vec::new();
    for conflict in decisions {
        let conflict_id = conflict.conflict_id.to_string();
        if existing.contains(&conflict_id) {
            continue;
        }
        let claim_a = conflict.claim_a.to_string();
        let claim_b = conflict.claim_b.to_string();
        let cache_key = cache_keys
            .get(&(claim_a.clone(), claim_b.clone()))
            .cloned()
            .unwrap_or_default();
        append_json(
            op,
            cx,
            <ConflictOpenedV2 as EventPayload>::KIND,
            &ConflictOpenedV2 {
                conflict_id: conflict_id.clone(),
                workspace_id: workspace_id.to_string(),
                claim_a: claim_a.clone(),
                claim_b: claim_b.clone(),
                reason: conflict.reason.clone(),
                detector: "texo-relate".to_string(),
                observed_at_ms,
                transition: transition_record(
                    CONFLICT_MACHINE,
                    &entity_for_conflict(&conflict_id),
                    0,
                    1,
                    vec![TransitionCauseV1 {
                        lane: 0,
                        key: format!("relate:{cache_key}"),
                    }],
                    observed_at_ms,
                ),
            },
        )?;
        rows.push(RelateConflictRow {
            conflict_id,
            claim_a,
            claim_b,
            reason: conflict.reason.clone(),
            cache_key,
        });
    }
    Ok(rows)
}
