use super::common::{
    append_json, assemble_current_view, op_runtime, parse_input, run_op, take_one_receipt,
    WORKSPACE_VIEW_PROJECTION,
};
use super::stats::conflict_phase_name;
use crate::claims::card::ClaimCard;
use crate::claims::conflict::ConflictCard;
use crate::claims::workspace::WorkspaceView;
use crate::error::TexoError;
use crate::events::coordinate::{entity_for_claim, entity_for_conflict};
use crate::events::machines::{transition_record, TransitionCauseV1, CONFLICT_MACHINE};
use crate::events::payloads::{ConflictOpenedV2, ConflictResolvedV2};
use crate::ops::env;
use crate::ops::env::ReceiptNote;
use crate::relate::heuristic;
use batpak::event::EventPayload;
use batpak::store::Freshness;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use syncbat::HandlerResult;

#[syncbat::operation(
    descriptor = CONFLICTS_LIST,
    register = register_conflicts_list,
    register_item = conflicts_list_item,
    name = "texo.conflicts.list",
    effect = Inspect,
    input_schema = "texo.conflicts.list.input.v2",
    output_schema = "texo.conflicts.list.output.v2",
    receipt_kind = "receipt.texo.conflicts.list.v2",
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
pub(super) fn conflicts_list(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.conflicts.list", || {
        let _input: ConflictsListInput = parse_input("texo.conflicts.list", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.conflicts.list", error))?;
        let view = assemble_current_view()?;
        Ok(conflicts_output(&view))
    })
}
#[syncbat::operation(
    descriptor = CONFLICTS_COMMIT,
    register = register_conflicts_commit,
    register_item = conflicts_commit_item,
    name = "texo.conflicts.commit",
    effect = Persist,
    input_schema = "texo.conflicts.commit.input.v2",
    output_schema = "texo.conflicts.commit.output.v2",
    receipt_kind = "receipt.texo.conflicts.commit.v2",
    appends_events = ["evt.e004"],
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
pub(super) fn conflicts_commit(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.conflicts.commit", || {
        let input: ConflictsCommitInput = parse_input("texo.conflicts.commit", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.conflicts.commit", error))?;
        let view = assemble_current_view()?;
        let detected = heuristic::detect_conflicts(&view)?;
        let existing = view
            .conflicts
            .iter()
            .map(|conflict| conflict.conflict_id.clone())
            .collect::<BTreeSet<_>>();
        let mut committed = Vec::new();
        for entry in detected.conflicts {
            if existing.contains(&entry.conflict_id) {
                continue;
            }
            let newer = newer_claim(&view, &entry.claim_a, &entry.claim_b)?;
            append_json(
                "texo.conflicts.commit",
                cx,
                <ConflictOpenedV2 as EventPayload>::KIND,
                &ConflictOpenedV2 {
                    conflict_id: entry.conflict_id.clone(),
                    workspace_id: view.workspace_id.clone(),
                    claim_a: entry.claim_a.clone(),
                    claim_b: entry.claim_b.clone(),
                    reason: entry.reason.clone(),
                    detector: "heuristic-v1".to_string(),
                    observed_at_ms: input.observed_at_ms,
                    transition: transition_record(
                        CONFLICT_MACHINE,
                        &entity_for_conflict(&entry.conflict_id),
                        0,
                        1,
                        vec![TransitionCauseV1 {
                            lane: 0,
                            key: format!("ingest:{}", newer.source_id),
                        }],
                        input.observed_at_ms,
                    ),
                },
            )?;
            let receipt = take_one_receipt("texo.conflicts.commit")?;
            committed.push(CommittedConflict {
                conflict_id: entry.conflict_id,
                sequence: receipt.global_sequence,
                receipt,
            });
        }
        Ok(committed)
    })
}
#[syncbat::operation(
    descriptor = CONFLICT_RESOLVE,
    register = register_conflict_resolve,
    register_item = conflict_resolve_item,
    name = "texo.conflict.resolve",
    effect = Persist,
    input_schema = "texo.conflict.resolve.input.v2",
    output_schema = "texo.conflict.resolve.output.v2",
    receipt_kind = "receipt.texo.conflict.resolve.v2",
    appends_events = ["evt.e006"],
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
pub(super) fn conflict_resolve(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.conflict.resolve", || {
        let input: ConflictResolveInput = parse_input("texo.conflict.resolve", input)?;
        if input.resolution != "resolved" && input.resolution != "ignored" {
            return Err(TexoError::StatusParse {
                value: input.resolution,
            });
        }
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.conflict.resolve", error))?;
        let entity = entity_for_conflict(&input.conflict_id);
        let card = env::with(|op_env| {
            env::deterministic_projection(|| {
                op_env
                    .store
                    .project::<ConflictCard>(&entity, &Freshness::Consistent)
            })
        })??;
        let card = card.ok_or_else(|| TexoError::MissingEntity {
            entity: entity.clone(),
        })?;
        let target_phase = if input.resolution == "resolved" { 2 } else { 3 };
        if card.phase == target_phase {
            return Ok(ConflictResolveOutput {
                conflict_id: input.conflict_id,
                resolution: input.resolution,
                already_applied: true,
                receipt: None,
            });
        }
        if card.phase != 1 {
            return Err(TexoError::Transition {
                machine: CONFLICT_MACHINE.to_string(),
                from: card.phase,
                to: target_phase,
                context: Some(format!(
                    "conflict {} is already {}",
                    input.conflict_id,
                    conflict_phase_name(card.phase)
                )),
            });
        }
        append_json(
            "texo.conflict.resolve",
            cx,
            <ConflictResolvedV2 as EventPayload>::KIND,
            &ConflictResolvedV2 {
                conflict_id: input.conflict_id.clone(),
                workspace_id: card.workspace_id,
                resolution: input.resolution.clone(),
                resolved_by: input.resolved_by,
                observed_at_ms: input.observed_at_ms,
                transition: transition_record(
                    CONFLICT_MACHINE,
                    &entity,
                    1,
                    target_phase,
                    vec![TransitionCauseV1 {
                        lane: 0,
                        key: format!("conflict:{}", input.conflict_id),
                    }],
                    input.observed_at_ms,
                ),
            },
        )?;
        Ok(ConflictResolveOutput {
            conflict_id: input.conflict_id,
            resolution: input.resolution,
            already_applied: false,
            receipt: Some(take_one_receipt("texo.conflict.resolve")?),
        })
    })
}
#[derive(Debug, Deserialize)]
struct ConflictsListInput {}

#[derive(Debug, Serialize)]
struct ConflictsOutput {
    open: Vec<ConflictRow>,
    resolved: Vec<ConflictRow>,
}

#[derive(Debug, Serialize)]
struct ConflictRow {
    conflict_id: String,
    claim_a: String,
    claim_b: String,
    subject_hint: String,
    reason: String,
    status: crate::claims::status::ConflictStatus,
}

#[derive(Debug, Deserialize)]
struct ConflictsCommitInput {
    observed_at_ms: u64,
}

#[derive(Debug, Serialize)]
struct CommittedConflict {
    conflict_id: String,
    sequence: u64,
    receipt: ReceiptNote,
}

#[derive(Debug, Deserialize)]
struct ConflictResolveInput {
    conflict_id: String,
    resolution: String,
    resolved_by: String,
    observed_at_ms: u64,
}

#[derive(Debug, Serialize)]
struct ConflictResolveOutput {
    conflict_id: String,
    resolution: String,
    already_applied: bool,
    receipt: Option<ReceiptNote>,
}
fn conflicts_output(view: &WorkspaceView) -> ConflictsOutput {
    let mut open = Vec::new();
    let mut resolved = Vec::new();
    for conflict in &view.conflicts {
        let row = ConflictRow {
            conflict_id: conflict.conflict_id.clone(),
            claim_a: conflict.claim_a.clone(),
            claim_b: conflict.claim_b.clone(),
            subject_hint: conflict_subject(view, conflict),
            reason: conflict.reason.clone(),
            status: conflict_status(conflict),
        };
        if conflict.phase == 1 {
            open.push(row);
        } else {
            resolved.push(row);
        }
    }
    open.sort_by(|left, right| left.conflict_id.cmp(&right.conflict_id));
    resolved.sort_by(|left, right| left.conflict_id.cmp(&right.conflict_id));
    ConflictsOutput { open, resolved }
}

pub(super) fn conflict_status(conflict: &ConflictCard) -> crate::claims::status::ConflictStatus {
    match conflict.phase {
        2 => crate::claims::status::ConflictStatus::Resolved,
        3 => crate::claims::status::ConflictStatus::Ignored,
        _ => crate::claims::status::ConflictStatus::Open,
    }
}
pub(super) fn conflict_subject(view: &WorkspaceView, conflict: &ConflictCard) -> String {
    view.claims
        .iter()
        .find(|claim| claim.card.claim_id == conflict.claim_a)
        .and_then(|claim| claim.card.subject_hint.clone())
        .unwrap_or_default()
}

pub(super) fn claim_text(view: &WorkspaceView, claim_id: &str) -> String {
    view.claims
        .iter()
        .find(|claim| claim.card.claim_id == claim_id)
        .map(|claim| claim.card.text.clone())
        .unwrap_or_default()
}

pub(super) fn newer_claim<'a>(
    view: &'a WorkspaceView,
    left: &str,
    right: &str,
) -> Result<&'a ClaimCard, TexoError> {
    let left = view
        .claims
        .iter()
        .find(|claim| claim.card.claim_id == left)
        .ok_or_else(|| TexoError::MissingEntity {
            entity: entity_for_claim(left),
        })?;
    let right = view
        .claims
        .iter()
        .find(|claim| claim.card.claim_id == right)
        .ok_or_else(|| TexoError::MissingEntity {
            entity: entity_for_claim(right),
        })?;
    if left.card.observed_at_ms >= right.card.observed_at_ms {
        Ok(&left.card)
    } else {
        Ok(&right.card)
    }
}
