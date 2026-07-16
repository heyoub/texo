use super::common::elapsed_ms;
use super::common::{op_runtime, parse_input, run_op, WORKSPACE_VIEW_PROJECTION};
use crate::claims::workspace::{assemble, WorkspaceView};
use crate::error::TexoError;
use crate::events::coordinate::scope_for_workspace;
use crate::events::machines::{CLAIM_EDGES, CLAIM_MACHINE, CONFLICT_EDGES, CONFLICT_MACHINE};
use crate::events::payloads::{
    ClaimEvidenceLinkedV1, ClaimRecordedV2, ClaimSupersededV2, ConflictOpenedV2,
    ConflictResolvedV2, EvidenceOccurrenceRecordedV1, EvidenceReconciliationAcceptedV1,
    OnboardingCompiledV2, RelationCampaignCheckpointV1, RelationDeferredV1, RelationJudgedV1,
    SessionTurnV1, SourceObservedV2, SourceSnapshotRecordedV1, SourceSnapshotRelationV1,
    WorkspaceInitializedV2,
};
use crate::ops::env;
use batpak::coordinate::Region;
use batpak::event::{EventKind, EventPayload};
use batpak::id::EntityIdType;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use syncbat::HandlerResult;

#[syncbat::operation(
    descriptor = VERIFY_RUN,
    register = register_verify_run,
    register_item = verify_run_item,
    name = "texo.verify.run",
    effect = Inspect,
    input_schema = "texo.verify.run.input.v2",
    output_schema = "texo.verify.run.output.v2",
    receipt_kind = "receipt.texo.verify.run.v2",
    reads_events = ["evt.e001", "evt.e002", "evt.e003", "evt.e004", "evt.e005", "evt.e006", "evt.e007", "evt.e008", "evt.e009", "evt.e00a", "evt.e00b", "evt.e00c", "evt.e00d", "evt.e00e", "evt.e00f", "evt.e010", "evt.e012"],
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn verify_run(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.verify.run", || {
        let _input: VerifyRunInput = parse_input("texo.verify.run", input)?;
        let replay_started = Instant::now();
        for kind in DOMAIN_KINDS {
            cx.event_read_handle()
                .read_event(format!("evt.{:04x}", kind.as_raw_u16()))
                .map_err(|error| op_runtime("texo.verify.run", error))?;
        }
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.verify.run", error))?;

        let mut errors = Vec::new();
        let (journal_ok, view, events_replayed) = env::with(|op_env| {
            let chain = op_env.store.verify_chain()?;
            let mut journal_ok = chain.is_intact();
            if !chain.is_intact() {
                errors.push(format!("chain: {chain:?}"));
            }
            let scope = scope_for_workspace(&op_env.workspace_id);
            let region = Region::scope(&scope);
            let mut after = None;
            let mut events_replayed = 0usize;
            loop {
                let page = op_env.store.query_entries_after(&region, after, 256);
                if page.is_empty() {
                    break;
                }
                for entry in &page {
                    events_replayed = events_replayed.saturating_add(1);
                    if !DOMAIN_KINDS.contains(&entry.event_kind()) {
                        journal_ok = false;
                        errors.push(format!(
                            "unknown event kind evt.{:04x} at {}",
                            entry.event_kind().as_raw_u16(),
                            entry.global_sequence()
                        ));
                    }
                    if let Err(error) = op_env.store.read_raw(entry.event_id()) {
                        journal_ok = false;
                        errors.push(format!(
                            "decode {}: {error}",
                            event_id_hex(entry.event_id())
                        ));
                    }
                }
                after = page.last().map(batpak::store::IndexEntry::global_sequence);
            }
            let mut cache = op_env.cache.borrow_mut();
            let view = env::deterministic_projection(|| {
                assemble(&op_env.store, &op_env.workspace_id, &mut cache)
            })?;
            Ok::<_, TexoError>((journal_ok, view, events_replayed))
        })??;

        let projection_ok = view
            .claims
            .iter()
            .all(|claim| claim.card.anomalies.is_empty())
            && view
                .conflicts
                .iter()
                .all(|conflict| conflict.anomalies.is_empty());
        if !projection_ok {
            errors.push("projection anomalies present".to_string());
        }
        let transitions_ok = validate_transition_edges(&view, &mut errors);

        Ok(VerifyRunOutput {
            projection_ok,
            journal_ok,
            transitions_ok,
            errors,
            replay_ms: elapsed_ms(replay_started),
            events_replayed,
        })
    })
}
const DOMAIN_KINDS: [EventKind; 17] = [
    <SourceObservedV2 as EventPayload>::KIND,
    <ClaimRecordedV2 as EventPayload>::KIND,
    <ClaimSupersededV2 as EventPayload>::KIND,
    <ConflictOpenedV2 as EventPayload>::KIND,
    <OnboardingCompiledV2 as EventPayload>::KIND,
    <ConflictResolvedV2 as EventPayload>::KIND,
    <WorkspaceInitializedV2 as EventPayload>::KIND,
    <SessionTurnV1 as EventPayload>::KIND,
    <RelationJudgedV1 as EventPayload>::KIND,
    <RelationDeferredV1 as EventPayload>::KIND,
    <SourceSnapshotRecordedV1 as EventPayload>::KIND,
    <EvidenceOccurrenceRecordedV1 as EventPayload>::KIND,
    <ClaimEvidenceLinkedV1 as EventPayload>::KIND,
    <crate::events::payloads::CodeIndexRecordedV1 as EventPayload>::KIND,
    <SourceSnapshotRelationV1 as EventPayload>::KIND,
    <EvidenceReconciliationAcceptedV1 as EventPayload>::KIND,
    <RelationCampaignCheckpointV1 as EventPayload>::KIND,
];
#[derive(Debug, Deserialize)]
struct VerifyRunInput {}

#[derive(Debug, Serialize)]
struct VerifyRunOutput {
    projection_ok: bool,
    journal_ok: bool,
    transitions_ok: bool,
    errors: Vec<String>,
    replay_ms: u64,
    events_replayed: usize,
}
fn validate_transition_edges(view: &WorkspaceView, errors: &mut Vec<String>) -> bool {
    let mut ok = true;
    for claim in &view.claims {
        let edge = match claim.card.phase {
            1 => Some((0, 1)),
            2 => Some((1, 2)),
            _ => None,
        };
        if edge.is_none_or(|edge| !CLAIM_EDGES.contains(&edge)) {
            ok = false;
            errors.push(format!(
                "claim {} invalid phase {} for {CLAIM_MACHINE}",
                claim.card.claim_id, claim.card.phase
            ));
        }
        if claim.card.phase == 2 && claim.card.superseded_by.is_none() {
            ok = false;
            errors.push(format!(
                "claim {} superseded without target",
                claim.card.claim_id
            ));
        }
    }
    for conflict in &view.conflicts {
        let edge = match conflict.phase {
            1 => Some((0, 1)),
            2 => Some((1, 2)),
            3 => Some((1, 3)),
            _ => None,
        };
        if edge.is_none_or(|edge| !CONFLICT_EDGES.contains(&edge)) {
            ok = false;
            errors.push(format!(
                "conflict {} invalid phase {} for {CONFLICT_MACHINE}",
                conflict.conflict_id, conflict.phase
            ));
        }
    }
    ok
}

fn event_id_hex(event_id: batpak::id::EventId) -> String {
    format!("{:032x}", event_id.as_u128())
}
