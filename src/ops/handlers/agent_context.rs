use super::claims::claim_list_rows;
use super::common::{
    assemble_snapshot_view, op_runtime, parse_input, run_op, WORKSPACE_VIEW_PROJECTION,
};
use super::conflicts::claim_text;
use super::model::AgentClaimRow;
use super::relate::require_complete_settlement;
use crate::claims::workspace::WorkspaceView;
use crate::error::TexoError;
use crate::knowledge::SnapshotRead;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use syncbat::HandlerResult;

#[syncbat::operation(
    descriptor = CONTEXT_AGENT,
    register = register_context_agent,
    register_item = context_agent_item,
    name = "texo.context.agent",
    effect = Inspect,
    input_schema = "texo.context.agent.input.v4",
    output_schema = "texo.context.agent.output.v3",
    receipt_kind = "receipt.texo.context.agent.v3",
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn context_agent(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.context.agent", || {
        let input: ContextAgentInput = parse_input("texo.context.agent", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.context.agent", error))?;
        let (view, snapshot) = assemble_snapshot_view(input.snapshot.as_deref())?;
        if !input.allow_unsettled {
            require_complete_settlement(&view)?;
        }
        build_agent_context_from_view(
            &view,
            input.subject.as_deref(),
            input.include_stale,
            snapshot,
        )
    })
}

#[derive(Debug, Deserialize)]
struct ContextAgentInput {
    subject: Option<String>,
    include_stale: bool,
    #[serde(default)]
    allow_unsettled: bool,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct AgentContextOutput {
    pub(super) workspace_id: String,
    pub(super) replayed_through_sequence: u64,
    pub(super) freshness: FreshnessView,
    pub(super) claims: Vec<AgentClaimRow>,
    pub(super) stale_claims: Vec<AgentStaleClaimRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) conflicts: Vec<AgentConflictRow>,
    pub(super) snapshot: SnapshotRead,
}

#[derive(Debug, Serialize)]
pub(super) struct FreshnessView {
    pub(super) kind: crate::claims::workspace::ProjectionFreshness,
    pub(super) description: String,
}

#[derive(Debug, Serialize)]
pub(super) struct AgentStaleClaimRow {
    pub(super) claim_id: String,
    pub(super) text: String,
    pub(super) superseded_by: String,
}

#[derive(Debug, Serialize)]
pub(super) struct AgentConflictRow {
    pub(super) conflict_id: String,
    pub(super) claim_a: String,
    pub(super) claim_a_text: String,
    pub(super) claim_b: String,
    pub(super) claim_b_text: String,
    pub(super) reason: String,
}

pub(super) fn build_agent_context_from_view(
    view: &WorkspaceView,
    subject: Option<&str>,
    include_stale: bool,
    snapshot: SnapshotRead,
) -> Result<AgentContextOutput, TexoError> {
    let claims = claim_list_rows(view, subject)?
        .into_iter()
        .filter(|claim| claim.status != crate::claims::status::ClaimStatus::Superseded)
        .collect::<Vec<_>>();
    let stale_claims = stale_claim_rows(view, subject, include_stale);
    let conflicts = conflict_rows(view);

    Ok(AgentContextOutput {
        workspace_id: view.workspace_id.clone(),
        replayed_through_sequence: view.frontier,
        freshness: FreshnessView {
            kind: view.freshness,
            description: format!(
                "Projection anchor validated through local store sequence {}. No global order or consensus is claimed.",
                view.frontier
            ),
        },
        claims,
        stale_claims,
        conflicts,
        snapshot,
    })
}

fn stale_claim_rows(
    view: &WorkspaceView,
    subject: Option<&str>,
    include_stale: bool,
) -> Vec<AgentStaleClaimRow> {
    if !include_stale {
        return Vec::new();
    }
    view.claims
        .iter()
        .filter(|claim| claim.card.phase == 2)
        .filter(|claim| {
            subject.is_none_or(|wanted| claim.card.subject_hint.as_deref() == Some(wanted))
        })
        .filter_map(|claim| {
            claim
                .card
                .superseded_by
                .as_ref()
                .map(|superseded_by| AgentStaleClaimRow {
                    claim_id: claim.card.claim_id.clone(),
                    text: claim.card.text.clone(),
                    superseded_by: superseded_by.clone(),
                })
        })
        .collect()
}

fn conflict_rows(view: &WorkspaceView) -> Vec<AgentConflictRow> {
    let mut conflicts = view
        .conflicts
        .iter()
        .filter(|conflict| conflict.phase == 1)
        .map(|conflict| AgentConflictRow {
            conflict_id: conflict.conflict_id.clone(),
            claim_a: conflict.claim_a.clone(),
            claim_a_text: claim_text(view, &conflict.claim_a),
            claim_b: conflict.claim_b.clone(),
            claim_b_text: claim_text(view, &conflict.claim_b),
            reason: conflict.reason.clone(),
        })
        .collect::<Vec<_>>();
    conflicts.sort_by(|left, right| left.conflict_id.cmp(&right.conflict_id));
    let mut seen_pairs = BTreeSet::new();
    conflicts.retain(|conflict| {
        let mut pair = [
            conflict.claim_a_text.to_ascii_lowercase(),
            conflict.claim_b_text.to_ascii_lowercase(),
        ];
        pair.sort();
        seen_pairs.insert(pair)
    });
    conflicts
}
