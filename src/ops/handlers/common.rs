use super::knowledge_read::{load_code_artifact_at, merge_coverage};
use super::model::AgentReceiptRow;
use crate::claims::evidence::{assemble_through as assemble_evidence_through, EvidenceProjection};
use crate::claims::temporal::{assemble_through as assemble_temporal_through, TemporalProjection};
use crate::claims::timeline::ClaimTimeline;
use crate::claims::workspace::{assemble, assemble_through, WorkspaceView};
use crate::error::{SnapshotFailureKind, TexoError};
use crate::events::coordinate::{entity_for_claim, scope_for_workspace};
use crate::events::ids::{ClaimId, WorkspaceId};
use crate::events::payloads::{
    ClaimRecordedV2, SourceSnapshotRecordedV1, SourceSnapshotRelationV1,
};
use crate::git_source::{compare_commits, CaptureLimits};
use crate::knowledge::{
    AnalysisQuality, CoverageGap, CoverageGapKind, EvidenceLinkMethod, EvidenceStance,
    KnowledgeCoverage, SnapshotDescriptor, SnapshotRead, SnapshotToken, TemporalRelation,
};
use crate::ops::env;
use crate::ops::env::ReceiptNote;
use crate::semantics::pipeline::RelateTemporalPolicy;
use batpak::coordinate::Region;
use batpak::event::EventSourced;
use batpak::event::{EventKind, EventPayload};
use batpak::id::EntityIdType;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;
use syncbat::{HandlerError, HandlerResult};

pub(super) const WORKSPACE_VIEW_PROJECTION: &str = "texo.workspace.view.v2";
const MAX_TEMPORAL_SNAPSHOT_COMPARISONS: usize = 1_024;
const MAX_GIT_ANCESTRY_WALK: usize = 100_000;

pub(super) fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn run_op<T: Serialize>(
    op: &'static str,
    f: impl FnOnce() -> Result<T, TexoError>,
) -> HandlerResult {
    let output = f()?;
    batpak::canonical::to_bytes(&output).map_err(|error| {
        HandlerError::from(TexoError::OpRuntime {
            op: op.to_string(),
            detail: error.to_string(),
            denied: false,
        })
    })
}

pub(crate) fn parse_input<T: serde::de::DeserializeOwned>(
    op: &str,
    input: &[u8],
) -> Result<T, TexoError> {
    batpak::canonical::from_bytes(input).map_err(|error| TexoError::OpInput {
        op: op.to_string(),
        detail: error.to_string(),
    })
}

pub(crate) fn append_json<T: Serialize>(
    op: &str,
    cx: &mut syncbat::Ctx<'_>,
    kind: EventKind,
    payload: &T,
) -> Result<(), TexoError> {
    let bytes = batpak::canonical::to_bytes(payload).map_err(|error| TexoError::OpRuntime {
        op: op.to_string(),
        detail: format!("canonical effect payload encoding failed: {error}"),
        denied: false,
    })?;
    cx.event_append_handle()
        .append_event(kind, &bytes)
        .map_err(|error| op_runtime(op, error))
}

pub(crate) fn take_receipts() -> Result<Vec<ReceiptNote>, TexoError> {
    env::with(|op_env| op_env.receipts.borrow_mut().drain(..).collect())
}

pub(crate) fn take_one_receipt(op: &str) -> Result<ReceiptNote, TexoError> {
    let mut receipts = take_receipts()?;
    receipts.pop().ok_or_else(|| TexoError::OpRuntime {
        op: op.to_string(),
        detail: "append produced no receipt".to_string(),
        denied: false,
    })
}

pub(crate) fn op_runtime(op: &str, error: impl std::fmt::Display) -> TexoError {
    TexoError::OpRuntime {
        op: op.to_string(),
        detail: error.to_string(),
        denied: false,
    }
}

pub(crate) fn config_error(error: crate::config::ConfigError) -> TexoError {
    TexoError::Config {
        detail: error.to_string(),
        source: Some(Box::new(error)),
    }
}

pub(crate) fn assemble_current_view() -> Result<std::sync::Arc<WorkspaceView>, TexoError> {
    env::with(|op_env| {
        let mut cache = op_env.cache.borrow_mut();
        env::deterministic_projection(|| assemble(&op_env.store, &op_env.workspace_id, &mut cache))
    })?
}

pub(crate) fn assemble_snapshot_view(
    requested: Option<&str>,
) -> Result<(std::sync::Arc<WorkspaceView>, SnapshotRead), TexoError> {
    let Some(requested) = requested else {
        let view = assemble_current_view()?;
        let snapshot = snapshot_for_view(&view)?;
        return Ok((view, snapshot));
    };
    let (store, workspace, journal_id) = env::with(|op_env| {
        (
            op_env.store.clone(),
            op_env.workspace_id.clone(),
            op_env.journal.id.clone(),
        )
    })?;
    let workspace_id = WorkspaceId::new(workspace.clone())?;
    let descriptor = SnapshotToken::resolve_for_journal(requested, &workspace_id, &journal_id)
        .map_err(|error| TexoError::Snapshot {
            kind: SnapshotFailureKind::InvalidToken,
            detail: error.to_string(),
        })?;
    let available_source =
        latest_source_snapshot(Some(descriptor.frontier))?.map(|snapshot| snapshot.snapshot_id);
    if available_source != descriptor.source_snapshot_id {
        return Err(TexoError::Snapshot {
            kind: SnapshotFailureKind::SourceUnavailable,
            detail: "the token's source snapshot is unavailable at its journal frontier"
                .to_string(),
        });
    }
    validate_snapshot_anchor(&store, &workspace, &descriptor)?;
    let view =
        env::deterministic_projection(|| assemble_through(&store, &workspace, descriptor.frontier))
            .map_err(|error| TexoError::Snapshot {
                kind: SnapshotFailureKind::Unavailable,
                detail: error.to_string(),
            })?;
    Ok((view, SnapshotRead::new(descriptor)))
}

pub(crate) fn snapshot_for_view(view: &WorkspaceView) -> Result<SnapshotRead, TexoError> {
    let workspace_id = WorkspaceId::new(view.workspace_id.clone())?;
    let (anchor_event_id_hex, journal_id) = env::with(|op_env| {
        Ok::<_, TexoError>((
            anchor_at_frontier(&op_env.store, &op_env.workspace_id, view.frontier)?,
            op_env.journal.id.clone(),
        ))
    })??;
    let source_snapshot_id =
        latest_source_snapshot(Some(view.frontier))?.map(|snapshot| snapshot.snapshot_id);
    Ok(SnapshotRead::new(SnapshotDescriptor {
        workspace_id,
        journal_id,
        frontier: view.frontier,
        anchor_event_id_hex,
        source_snapshot_id,
    }))
}

pub(crate) fn validate_snapshot_anchor(
    store: &crate::journal_store::JournalStore,
    workspace_id: &str,
    descriptor: &SnapshotDescriptor,
) -> Result<(), TexoError> {
    let actual = anchor_at_frontier(store, workspace_id, descriptor.frontier)?;
    if actual != descriptor.anchor_event_id_hex {
        return Err(TexoError::Snapshot {
            kind: SnapshotFailureKind::AnchorMismatch,
            detail: format!(
                "journal anchor at frontier {} differs from the token",
                descriptor.frontier
            ),
        });
    }
    Ok(())
}

pub(crate) fn anchor_at_frontier(
    store: &crate::journal_store::JournalStore,
    workspace_id: &str,
    frontier: u64,
) -> Result<String, TexoError> {
    if frontier == 0 {
        return Ok(String::new());
    }
    let region = Region::scope(scope_for_workspace(workspace_id));
    let entry = store
        .query_entries_after(&region, Some(frontier.saturating_sub(1)), 1)
        .into_iter()
        .next()
        .filter(|entry| entry.global_sequence() == frontier)
        .ok_or_else(|| TexoError::Snapshot {
            kind: SnapshotFailureKind::Unavailable,
            detail: format!("workspace frontier {frontier} is unavailable"),
        })?;
    Ok(format!("{:032x}", entry.event_id().as_u128()))
}

pub(crate) fn claim_timeline_through(
    entity: &str,
    frontier: u64,
) -> Result<ClaimTimeline, TexoError> {
    env::with(|op_env| {
        let mut timeline = ClaimTimeline::default();
        for entry in op_env.store.by_entity(entity) {
            if entry.global_sequence() > frontier {
                break;
            }
            let raw = op_env.store.read_raw(entry.event_id())?;
            timeline.apply_event(&raw.event);
        }
        Ok::<_, TexoError>(timeline)
    })?
}

pub(crate) fn coverage_for_view(
    view: &WorkspaceView,
    snapshot: &SnapshotRead,
) -> Result<KnowledgeCoverage, TexoError> {
    // Only a genuinely missing snapshot (`Ok(None)`) or a mismatched identity
    // degrades to `Unavailable`; a decode/corruption error must bubble up rather
    // than masquerade as an ordinary absent snapshot.
    if let Some(recorded) = latest_source_snapshot(Some(view.frontier))? {
        if Some(&recorded.snapshot_id) == snapshot.descriptor.source_snapshot_id.as_ref() {
            return Ok(recorded.coverage);
        }
    }
    Ok(KnowledgeCoverage {
        analysis_quality: AnalysisQuality::Unavailable,
        sources_examined: u64::try_from(view.sources.len()).unwrap_or(u64::MAX),
        occurrences: u64::try_from(view.claims.len()).unwrap_or(u64::MAX),
        truncated: false,
        gaps: vec![CoverageGap {
            path: None,
            kind: CoverageGapKind::SourceSnapshotUnavailable,
        }],
    })
}

pub(crate) fn status_coverage(
    view: &WorkspaceView,
    snapshot: &SnapshotRead,
) -> Result<(KnowledgeCoverage, bool), TexoError> {
    let mut coverage = coverage_for_view(view, snapshot)?;
    let Some(source_snapshot_id) = snapshot.descriptor.source_snapshot_id.as_ref() else {
        return Ok((coverage, false));
    };
    let loaded = load_code_artifact_at(view.frontier, source_snapshot_id)?;
    if let Some(code_coverage) = loaded.coverage {
        merge_coverage(&mut coverage, &code_coverage);
    }
    if loaded.unavailable {
        coverage.gaps.push(CoverageGap {
            path: None,
            kind: CoverageGapKind::CodeIndexUnavailable,
        });
    }
    Ok((coverage, !loaded.unavailable))
}

/// Reconstruct the capture bounds a snapshot was recorded with so code indexing
/// recaptures the same bounded world instead of the defaults.
pub(crate) fn recorded_capture_limits(recorded: &SourceSnapshotRecordedV1) -> CaptureLimits {
    CaptureLimits {
        max_files: usize::try_from(recorded.capture_max_files).unwrap_or(usize::MAX),
        max_file_bytes: recorded.capture_max_file_bytes,
        max_total_bytes: recorded.capture_max_total_bytes,
    }
}

pub(crate) fn latest_source_snapshot(
    frontier: Option<u64>,
) -> Result<Option<SourceSnapshotRecordedV1>, TexoError> {
    Ok(source_snapshots_through(frontier)?.pop())
}

/// The recorded snapshot with this content-addressed id, regardless of whether a
/// newer snapshot was recorded afterward (revisiting a prior commit re-derives an
/// existing id rather than appending a new record).
pub(crate) fn source_snapshot_by_id(
    snapshot_id: &crate::knowledge::SourceSnapshotId,
) -> Result<Option<SourceSnapshotRecordedV1>, TexoError> {
    Ok(source_snapshots_through(None)?
        .into_iter()
        .rev()
        .find(|snapshot| &snapshot.snapshot_id == snapshot_id))
}

pub(crate) fn source_snapshots_through(
    frontier: Option<u64>,
) -> Result<Vec<SourceSnapshotRecordedV1>, TexoError> {
    env::with(|op_env| {
        let region = Region::scope(scope_for_workspace(&op_env.workspace_id));
        let mut after = None;
        let mut snapshots = Vec::new();
        'pages: loop {
            let page = op_env.store.query_entries_after(&region, after, 256);
            if page.is_empty() {
                break;
            }
            for entry in &page {
                if frontier.is_some_and(|frontier| entry.global_sequence() > frontier) {
                    break 'pages;
                }
                if entry.event_kind() == <SourceSnapshotRecordedV1 as EventPayload>::KIND {
                    let raw = op_env.store.read_raw(entry.event_id())?;
                    snapshots.push(
                        batpak::encoding::from_bytes::<SourceSnapshotRecordedV1>(
                            &raw.event.payload,
                        )
                        .map_err(|error| TexoError::Decode {
                            entity: entry.coord().entity().to_string(),
                            detail: error.to_string(),
                        })?,
                    );
                }
            }
            after = page.last().map(batpak::store::IndexEntry::global_sequence);
        }
        Ok::<_, TexoError>(snapshots)
    })?
}

pub(crate) fn plan_snapshot_relations(
    root: &Path,
    workspace_id: &WorkspaceId,
    capture: &crate::git_source::GitCapture,
    previous: &[SourceSnapshotRecordedV1],
    observed_at_ms: u64,
) -> Result<(Vec<SourceSnapshotRelationV1>, Vec<CoverageGap>), TexoError> {
    let skipped = previous
        .len()
        .saturating_sub(MAX_TEMPORAL_SNAPSHOT_COMPARISONS);
    let mut gaps = Vec::new();
    if skipped > 0 {
        gaps.push(CoverageGap {
            path: None,
            kind: CoverageGapKind::BudgetExceeded,
        });
    }
    let mut relations = Vec::new();
    for prior in previous.iter().skip(skipped) {
        if prior.snapshot_id == capture.snapshot_id {
            continue;
        }
        let comparison = if prior.repository_id == capture.repository_id {
            overlay_aware_comparison(root, prior, capture)?
        } else {
            crate::git_source::GitComparison {
                relation: TemporalRelation::Unknown,
                gap: Some(CoverageGapKind::MissingObject),
            }
        };
        if let Some(kind) = comparison.gap {
            let gap = CoverageGap { path: None, kind };
            if !gaps.contains(&gap) {
                gaps.push(gap);
            }
        }
        // Never journal an `Unknown` ordering: the relation idempotency key is
        // (workspace, left, right) and replay keeps the first fact, so a durable
        // Unknown from shallow history or an exhausted walk would permanently
        // shadow the real Before/After discoverable once full history arrives.
        // Its absence already reads as Unknown, and the gap above records why.
        if comparison.relation == TemporalRelation::Unknown {
            continue;
        }
        relations.push(SourceSnapshotRelationV1 {
            workspace_id: workspace_id.clone(),
            repository_id: capture.repository_id.clone(),
            left_snapshot_id: prior.snapshot_id.clone(),
            right_snapshot_id: capture.snapshot_id.clone(),
            left_commit: prior.base_commit.clone(),
            right_commit: capture.base_commit.clone(),
            relation: comparison.relation,
            observed_at_ms,
        });
    }
    Ok((relations, gaps))
}

pub(crate) fn overlay_aware_comparison(
    root: &Path,
    prior: &SourceSnapshotRecordedV1,
    capture: &crate::git_source::GitCapture,
) -> Result<crate::git_source::GitComparison, TexoError> {
    use crate::git_source::GitComparison;

    if prior.base_commit == capture.base_commit {
        return Ok(GitComparison {
            relation: match (prior.dirty, capture.dirty) {
                (false, true) => TemporalRelation::Before,
                (true, false) => TemporalRelation::After,
                (true, true) => TemporalRelation::Concurrent,
                (false, false) => TemporalRelation::Same,
            },
            gap: None,
        });
    }
    let comparison = compare_commits(
        root,
        &prior.base_commit,
        &capture.base_commit,
        MAX_GIT_ANCESTRY_WALK,
    )?;
    let relation = match comparison.relation {
        TemporalRelation::Before if prior.dirty => TemporalRelation::Concurrent,
        TemporalRelation::After if capture.dirty => TemporalRelation::Concurrent,
        TemporalRelation::Same => TemporalRelation::Same,
        TemporalRelation::Before => TemporalRelation::Before,
        TemporalRelation::After => TemporalRelation::After,
        TemporalRelation::Concurrent => TemporalRelation::Concurrent,
        TemporalRelation::Unknown => TemporalRelation::Unknown,
    };
    Ok(GitComparison {
        relation,
        gap: comparison.gap,
    })
}

pub(crate) fn plan_and_attach_snapshot_relations(
    root: &Path,
    workspace_id: &WorkspaceId,
    capture: &mut crate::git_source::GitCapture,
    previous: &[SourceSnapshotRecordedV1],
    observed_at_ms: u64,
) -> Result<Vec<SourceSnapshotRelationV1>, TexoError> {
    let (relations, gaps) =
        plan_snapshot_relations(root, workspace_id, capture, previous, observed_at_ms)?;
    for gap in gaps {
        if capture.coverage.gaps.len() < 256 && !capture.coverage.gaps.contains(&gap) {
            capture.coverage.gaps.push(gap);
        }
    }
    Ok(relations)
}

pub(crate) fn evidence_projection_through(frontier: u64) -> Result<EvidenceProjection, TexoError> {
    env::with(|op_env| {
        env::deterministic_projection(|| {
            assemble_evidence_through(&op_env.store, &op_env.workspace_id, frontier)
        })
    })?
}

pub(crate) fn temporal_projection_through(frontier: u64) -> Result<TemporalProjection, TexoError> {
    env::with(|op_env| {
        env::deterministic_projection(|| {
            assemble_temporal_through(&op_env.store, &op_env.workspace_id, frontier)
        })
    })?
}

pub(crate) fn workspace_temporal_policy(
    view: &WorkspaceView,
) -> Result<RelateTemporalPolicy, TexoError> {
    workspace_temporal_policy_through(view, view.frontier)
}

pub(crate) fn workspace_temporal_policy_through(
    view: &WorkspaceView,
    frontier: u64,
) -> Result<RelateTemporalPolicy, TexoError> {
    let evidence = evidence_projection_through(frontier)?;
    let relations = temporal_projection_through(frontier)?;
    let mut policy = RelateTemporalPolicy::default();
    for claim in &view.claims {
        let claim_id = ClaimId::try_from(claim.card.claim_id.as_str())?;
        if let Some(latest) = evidence
            .for_claim(claim_id.as_str())
            .iter()
            .filter(|item| {
                item.method == EvidenceLinkMethod::Deterministic
                    && item.stance == EvidenceStance::Supports
            })
            .max_by_key(|item| item.link_sequence)
        {
            policy.bind_claim(&claim_id, &latest.occurrence.snapshot_id);
        }
    }
    for (left, right, relation) in relations.facts() {
        policy.insert_relation_ids(left, right, relation);
    }
    Ok(policy)
}

pub(crate) fn semantic_temporal_policy(
    view: &WorkspaceView,
) -> Result<RelateTemporalPolicy, TexoError> {
    workspace_temporal_policy(view)
}
pub(crate) fn claim_record_receipts() -> Result<BTreeMap<String, AgentReceiptRow>, TexoError> {
    env::with(|op_env| {
        let scope = scope_for_workspace(&op_env.workspace_id);
        let mut receipts = BTreeMap::new();
        for entry in op_env.store.by_scope(&scope) {
            if entry.event_kind() != <ClaimRecordedV2 as EventPayload>::KIND {
                continue;
            }
            // The entity coordinate is "claim:{claim_id}" — the payload decode
            // it used to do here (read_raw + msgpack per event) carried no
            // information the index entry does not already hold.
            let Some(claim_id) = entry.coord().entity().strip_prefix("claim:") else {
                continue;
            };
            receipts
                .entry(claim_id.to_string())
                .or_insert(AgentReceiptRow {
                    event_id: event_id_hex(entry.event_id()),
                    sequence: entry.global_sequence(),
                });
        }
        Ok::<_, TexoError>(receipts)
    })?
}

fn event_id_hex(event_id: batpak::id::EventId) -> String {
    format!("{:032x}", event_id.as_u128())
}

pub(crate) fn claim_receipt(claim_id: &str) -> Result<AgentReceiptRow, TexoError> {
    let entity = entity_for_claim(claim_id);
    env::with(|op_env| {
        let entry = op_env
            .store
            .by_entity(&entity)
            .into_iter()
            .find(|entry| entry.event_kind() == <ClaimRecordedV2 as EventPayload>::KIND)
            .ok_or_else(|| TexoError::MissingEntity {
                entity: entity.clone(),
            })?;
        Ok::<_, TexoError>(AgentReceiptRow {
            event_id: event_id_hex(entry.event_id()),
            sequence: entry.global_sequence(),
        })
    })?
}
