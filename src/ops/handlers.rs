//! First texo operation handlers.
#![expect(
    missing_docs,
    reason = "syncbat::operation generates public registration shims without doc injection hooks"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use batpak::coordinate::Region;
use batpak::event::{EventKind, EventPayload, EventSourced};
use batpak::id::EntityIdType;
use batpak::store::Freshness;
use serde::{Deserialize, Serialize};
use syncbat::{CoreBuilder, HandlerError, HandlerResult, OperationRegisterItem};

use crate::claims::card::ClaimCard;
use crate::claims::conflict::ConflictCard;
use crate::claims::evidence::{assemble_through as assemble_evidence_through, EvidenceProjection};
use crate::claims::temporal::{assemble_through as assemble_temporal_through, TemporalProjection};
use crate::claims::timeline::{ClaimTimeline, TimelineEntry};
use crate::claims::workspace::{assemble, assemble_through, ClaimView, WorkspaceView};
use crate::code_index::{
    build as build_code_index, load as load_code_index, persist as persist_code_index, read_scip,
    CodeIndexLimits,
};
use crate::config::{TexoRootConfig, WorkspaceEntry};
use crate::error::{SnapshotFailureKind, TexoError};
use crate::events::coordinate::{
    entity_for_claim, entity_for_conflict, entity_for_workspace_meta, scope_for_workspace,
};
use crate::events::ids::{claim_id_from_parts, ClaimId, SourceId, WorkspaceId};
use crate::events::machines::{
    transition_record, TransitionCauseV1, CLAIM_EDGES, CLAIM_MACHINE, CONFLICT_EDGES,
    CONFLICT_MACHINE,
};
use crate::events::payloads::{
    ClaimEvidenceLinkedV1, ClaimRecordedV2, ClaimSupersededV2, CodeIndexRecordedV1,
    ConflictOpenedV2, ConflictResolvedV2, EvidenceOccurrenceRecordedV1,
    EvidenceReconciliationAcceptedV1, OnboardingCompiledV2, RelationDeferredV1, RelationJudgedV1,
    SessionTurnV1, SourceObservedV2, SourceSnapshotRecordedV1, SourceSnapshotRelationV1,
    WorkspaceInitializedV2,
};
use crate::extract::hints::hints_from_line_normalized;
use crate::extract::markdown::{collect_markdown_files, MarkdownDocument};
use crate::extract::normalize::normalize_line;
use crate::git_source::{
    capture as capture_git, compare_commits, CaptureLimits, CapturedLayer, CapturedSource,
};
use crate::knowledge::{
    AnalysisQuality, AnswerState, ByteRange, ClaimEvidence, CodeIndexArtifact, CodeIndexId,
    CodeOccurrence, CoverageGap, CoverageGapKind, EvidenceLinkMethod, EvidenceOccurrence,
    EvidenceOccurrenceId, EvidenceSourceKind, EvidenceStance, KnowledgeCoverage, LineRange,
    RepositoryId, SnapshotDescriptor, SnapshotRead, SnapshotToken, TemporalRelation,
    TriangulationTarget, UncertaintyReason, MAX_EVIDENCE_EXCERPT_BYTES,
};
use crate::ops::env::{self, ReceiptNote};
use crate::ops::reconcile::append_proposals as append_reconciliation_proposals;
use crate::reconcile::{
    claims_from_view as reconcile_claims, evaluate_with_backends, plan_candidates,
    unresolved_row as reconcile_unresolved_row, KnowledgeReconcileInput, KnowledgeReconcileOutput,
    ReconcileBackendOutput, ReconcileCompletion,
};
use crate::relate::heuristic;
use crate::semantics::pipeline::{
    receipt_view, ClaimStatus as SemanticClaimStatus, ClaimView as SemanticClaimView,
    ParallelRelateOptions, RelateTemporalPolicy, RelateThresholds,
};

const WORKSPACE_VIEW_PROJECTION: &str = "texo.workspace.view.v2";
const CLAIM_EXPLAIN_PROJECTION: &str = "texo.claim.explain.v2";
const RELATE_PREFILTER: f32 = 0.60;
const MAX_INLINE_SOURCE_FAILURES: usize = 256;
const MAX_SOURCE_FAILURE_DETAIL_CHARS: usize = 512;
const MAX_TRIANGULATION_CODE_OCCURRENCES: usize = 200;
const MAX_TEMPORAL_SNAPSHOT_COMPARISONS: usize = 1_024;
const MAX_GIT_ANCESTRY_WALK: usize = 100_000;
#[cfg(feature = "openrouter")]
const ENV_RELATE_CACHE: &str = "TEXO_RELATE_CACHE";
#[cfg(feature = "openrouter")]
const DEFAULT_RELATE_CACHE: &str = ".texo/relate-cache";

#[syncbat::operation(
    descriptor = WORKSPACE_INIT,
    register = register_workspace_init,
    register_item = workspace_init_item,
    name = "texo.workspace.init",
    effect = Persist,
    input_schema = "texo.workspace.init.input.v2",
    output_schema = "texo.workspace.init.output.v2",
    receipt_kind = "receipt.texo.workspace.init.v2",
    appends_events = ["evt.e007"]
)]
#[tracing::instrument(skip_all)]
fn workspace_init(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.workspace.init", || {
        let input: WorkspaceInitInput = parse_input("texo.workspace.init", input)?;
        let (root, observed_at_ms) =
            env::with(|op_env| (op_env.root.clone(), op_env.observed_at_ms))?;
        let config_path = root.join(".texo").join("config.toml");

        let mut root_config = if config_path.exists() {
            TexoRootConfig::load(&config_path).map_err(config_error)?
        } else {
            TexoRootConfig {
                default_workspace: input.workspace_id.clone(),
                workspaces: BTreeMap::new(),
                gateway: None,
            }
        };
        root_config
            .default_workspace
            .clone_from(&input.workspace_id);
        root_config.upsert_workspace(
            &input.workspace_id,
            WorkspaceEntry::for_id(&input.workspace_id),
        );

        let raw = toml::to_string_pretty(&root_config).map_err(|error| TexoError::Config {
            detail: error.to_string(),
            source: Some(Box::new(error)),
        })?;
        let config_unchanged = std::fs::read(&config_path)
            .ok()
            .is_some_and(|existing| existing == raw.as_bytes());
        if !config_unchanged {
            if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&config_path, raw.as_bytes())?;
        }
        let config_digest_hex = blake3::hash(raw.as_bytes()).to_hex().to_string();
        let journal_digest_matches = env::with(|op_env| {
            let entity = entity_for_workspace_meta(&input.workspace_id);
            let mut entries = op_env.store.by_entity(&entity);
            entries.sort_by_key(batpak::store::IndexEntry::global_sequence);
            let Some(entry) = entries.last() else {
                return Ok::<_, TexoError>(false);
            };
            let raw = op_env.store.read_raw(entry.event_id())?;
            let payload: WorkspaceInitializedV2 = batpak::encoding::from_bytes(&raw.event.payload)
                .map_err(|error| TexoError::Decode {
                    entity,
                    detail: error.to_string(),
                })?;
            Ok(payload.config_digest_hex == config_digest_hex)
        })??;
        let already_initialized = config_unchanged && journal_digest_matches;

        append_json(
            "texo.workspace.init",
            cx,
            <WorkspaceInitializedV2 as EventPayload>::KIND,
            &WorkspaceInitializedV2 {
                workspace_id: input.workspace_id.clone(),
                schema: "texo.v2".to_string(),
                config_digest_hex,
                created_at_ms: observed_at_ms,
            },
        )?;
        let mut receipts = take_receipts()?;
        let receipt = receipts.pop().ok_or_else(|| TexoError::OpRuntime {
            op: "texo.workspace.init".to_string(),
            detail: "workspace init append produced no receipt".to_string(),
            denied: false,
        })?;

        Ok(WorkspaceInitOutput {
            workspace_id: input.workspace_id,
            config_path: config_path.to_string_lossy().to_string(),
            already_initialized,
            receipt,
        })
    })
}

#[syncbat::operation(
    descriptor = INGEST_RUN,
    register = register_ingest_run,
    register_item = ingest_run_item,
    name = "texo.ingest.run",
    effect = Persist,
    input_schema = "texo.ingest.run.input.v2",
    output_schema = "texo.ingest.run.output.v2",
    receipt_kind = "receipt.texo.ingest.run.v2",
    appends_events = ["evt.e001", "evt.e002", "evt.e003"],
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
#[expect(
    clippy::too_many_lines,
    reason = "ingest planning and append phases stay visibly separated to prove strict atomicity"
)]
fn ingest_run(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.ingest.run", || {
        let input: IngestRunInput = parse_input("texo.ingest.run", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.ingest.run", error))?;
        let (root, workspace_id, config) = env::with(|op_env| {
            (
                op_env.root.clone(),
                op_env.workspace_id.clone(),
                op_env.config.clone(),
            )
        })?;
        let project_started = Instant::now();
        let mut view = assemble_current_view()?;
        let mut project_ms = elapsed_ms(project_started);
        let path = resolve_path(&root, &input.path);
        let plan = plan_sources(
            "texo.ingest.run",
            &root,
            &path,
            &workspace_id,
            input.observed_at_ms,
            config.extractor_cmd.as_deref(),
            &view,
        )?;
        if !plan.skipped.is_empty() && (input.strict || plan.succeeded == 0 && !plan.empty) {
            let sample =
                serde_json::to_string(&plan.skipped.iter().take(8).cloned().collect::<Vec<_>>())?;
            let (_, artifact) =
                settle_source_failures(&root, input.observed_at_ms, plan.skipped.clone())?;
            return Err(TexoError::Source {
                path: path.to_string_lossy().to_string(),
                detail: format!(
                    "{} source(s) failed during planning; strict={} good_sources={}; sample={sample}; artifact={}",
                    plan.skipped.len(),
                    input.strict,
                    plan.succeeded,
                    artifact.as_deref().unwrap_or("inline")
                ),
            });
        }

        let mut source_count = 0_u32;
        let mut claim_count = 0_u32;
        let mut supersede_count = 0_u32;
        let mut append_ms = 0_u64;

        if input.dry_run {
            for source in &plan.sources {
                source_count = source_count.saturating_add(1);
                let planned = u32::try_from(source.claims.len()).unwrap_or(u32::MAX);
                claim_count = claim_count.saturating_add(planned);
            }
        } else {
            let append_started = Instant::now();
            for source in &plan.sources {
                append_json(
                    "texo.ingest.run",
                    cx,
                    <SourceObservedV2 as EventPayload>::KIND,
                    &source.observed,
                )?;
                source_count = source_count.saturating_add(1);
                for claim in &source.claims {
                    append_json(
                        "texo.ingest.run",
                        cx,
                        <ClaimRecordedV2 as EventPayload>::KIND,
                        claim,
                    )?;
                    claim_count = claim_count.saturating_add(1);
                }
            }
            append_ms = append_ms.saturating_add(elapsed_ms(append_started));

            let project_started = Instant::now();
            view = assemble_current_view()?;
            project_ms = project_ms.saturating_add(elapsed_ms(project_started));
            if !config
                .semantics
                .as_ref()
                .is_some_and(|semantics| semantics.enabled)
            {
                let new_claims = plan
                    .sources
                    .iter()
                    .flat_map(|source| source.claims.iter().cloned())
                    .collect::<Vec<_>>();
                let append_started = Instant::now();
                for superseded in infer_supersessions(&view, &new_claims, input.observed_at_ms) {
                    append_json(
                        "texo.ingest.run",
                        cx,
                        <ClaimSupersededV2 as EventPayload>::KIND,
                        &superseded,
                    )?;
                    supersede_count = supersede_count.saturating_add(1);
                }
                append_ms = append_ms.saturating_add(elapsed_ms(append_started));
            }
        }

        let outcome = if plan.skipped.is_empty() {
            IngestCompletion::Complete
        } else {
            IngestCompletion::Partial
        };
        let skipped_total = plan.skipped.len();
        let (skipped, skipped_artifact) =
            settle_source_failures(&root, input.observed_at_ms, plan.skipped)?;
        Ok(IngestRunOutput {
            outcome,
            workspace_id,
            sources_observed: source_count,
            claims_recorded: claim_count,
            claims_superseded: supersede_count,
            dry_run: input.dry_run,
            empty: plan.empty,
            skipped,
            skipped_total,
            skipped_artifact,
            phase_ms: IngestPhaseMs {
                discover: plan.discover_ms,
                extract: plan.extract_ms,
                append: append_ms,
                project: project_ms,
            },
            events_appended: if input.dry_run {
                0
            } else {
                u64::from(source_count)
                    .saturating_add(u64::from(claim_count))
                    .saturating_add(u64::from(supersede_count))
            },
            receipts: if input.dry_run {
                Vec::new()
            } else {
                take_receipts()?
            },
        })
    })
}

#[syncbat::operation(
    descriptor = CLAIMS_LIST,
    register = register_claims_list,
    register_item = claims_list_item,
    name = "texo.claims.list",
    effect = Inspect,
    input_schema = "texo.claims.list.input.v3",
    output_schema = "texo.claims.list.output.v3",
    receipt_kind = "receipt.texo.claims.list.v3",
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn claims_list(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.claims.list", || {
        let input: ClaimsListInput = parse_input("texo.claims.list", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.claims.list", error))?;
        let (view, snapshot) = assemble_snapshot_view(input.snapshot.as_deref())?;
        let claims = claim_list_rows(&view, input.subject.as_deref())?;
        Ok(ClaimsListOutput {
            workspace_id: view.workspace_id.clone(),
            frontier: view.frontier,
            claims,
            snapshot,
        })
    })
}

#[syncbat::operation(
    descriptor = CLAIMS_SEARCH,
    register = register_claims_search,
    register_item = claims_search_item,
    name = "texo.claims.search",
    effect = Inspect,
    input_schema = "texo.claims.search.input.v2",
    output_schema = "texo.claims.search.output.v2",
    receipt_kind = "receipt.texo.claims.search.v2",
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn claims_search(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.claims.search", || {
        let input: ClaimsSearchInput = parse_input("texo.claims.search", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.claims.search", error))?;
        let (view, snapshot) = assemble_snapshot_view(input.snapshot.as_deref())?;
        let query = input.query.unwrap_or_default();
        if query.len() > 256 {
            return Err(TexoError::OpInput {
                op: "texo.claims.search".to_string(),
                detail: "query exceeds 256 bytes".to_string(),
            });
        }
        let limit = input.limit.unwrap_or(25);
        if !(1..=100).contains(&limit) {
            return Err(TexoError::OpInput {
                op: "texo.claims.search".to_string(),
                detail: "limit must be between 1 and 100".to_string(),
            });
        }
        let offset = parse_claim_search_cursor(input.cursor.as_deref())?;
        let query_terms = query
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        let rows = claim_list_rows(&view, input.subject.as_deref())?
            .into_iter()
            .filter(|row| input.status.is_none_or(|status| row.status == status))
            .filter(|row| claim_matches_query(row, &query_terms))
            .collect::<Vec<_>>();
        let total = rows.len();
        let page = rows
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        let returned = page.len();
        let next_offset = offset.saturating_add(returned);
        let has_more = next_offset < total;
        Ok(ClaimsSearchOutput {
            workspace_id: view.workspace_id.clone(),
            frontier: view.frontier,
            freshness: view.freshness,
            total,
            returned,
            has_more,
            next_cursor: has_more.then(|| format!("texo-claims-v1:{next_offset}")),
            claims: page,
            snapshot,
        })
    })
}

#[syncbat::operation(
    descriptor = KNOWLEDGE_SEARCH,
    register = register_knowledge_search,
    register_item = knowledge_search_item,
    name = "texo.knowledge.search",
    effect = Inspect,
    input_schema = "texo.knowledge.search.input.v1",
    output_schema = "texo.knowledge.search.output.v1",
    receipt_kind = "receipt.texo.knowledge.search.v1",
    reads_events = ["evt.e00e"],
    queries_projections = ["texo.workspace.view.v2", "texo.code.index.v1"]
)]
#[tracing::instrument(skip_all)]
fn knowledge_search(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.knowledge.search", || {
        let input: KnowledgeSearchInput = parse_input("texo.knowledge.search", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.knowledge.search", error))?;
        let (view, snapshot) = assemble_snapshot_view(input.snapshot.as_deref())?;
        search_knowledge_from_view(&view, snapshot, &input)
    })
}

#[syncbat::operation(
    descriptor = CLAIM_EXPLAIN,
    register = register_claim_explain,
    register_item = claim_explain_item,
    name = "texo.claim.explain",
    effect = Inspect,
    input_schema = "texo.claim.explain.input.v3",
    output_schema = "texo.claim.explain.output.v4",
    receipt_kind = "receipt.texo.claim.explain.v4",
    queries_projections = ["texo.claim.explain.v2"]
)]
#[tracing::instrument(skip_all)]
fn claim_explain(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.claim.explain", || {
        let input: ClaimExplainInput = parse_input("texo.claim.explain", input)?;
        cx.projection_read_handle()
            .query_projection(CLAIM_EXPLAIN_PROJECTION)
            .map_err(|error| op_runtime("texo.claim.explain", error))?;
        let entity = entity_for_claim(&input.claim_id);
        let (view, snapshot) = assemble_snapshot_view(input.snapshot.as_deref())?;
        let card = view
            .claims
            .iter()
            .find(|claim| claim.card.claim_id == input.claim_id)
            .map(|claim| claim.card.as_ref().clone())
            .ok_or_else(|| TexoError::MissingEntity {
                entity: entity.clone(),
            })?;
        let timeline = claim_timeline_through(&entity, view.frontier)?;
        let evidence = evidence_projection_through(view.frontier)?.take_claim(&input.claim_id);
        let coverage = coverage_for_view(&view, &snapshot);
        let answer_state = answer_state_for_claim(
            view.claims
                .iter()
                .find(|claim| claim.card.claim_id == input.claim_id)
                .map(|claim| claim.status),
            &evidence,
        );
        Ok(ClaimExplainOutput {
            card,
            timeline: timeline.entries,
            answer_state,
            evidence,
            coverage,
            snapshot,
        })
    })
}

#[syncbat::operation(
    descriptor = KNOWLEDGE_TRIANGULATE,
    register = register_knowledge_triangulate,
    register_item = knowledge_triangulate_item,
    name = "texo.knowledge.triangulate",
    effect = Inspect,
    input_schema = "texo.knowledge.triangulate.input.v1",
    output_schema = "texo.knowledge.triangulate.output.v1",
    receipt_kind = "receipt.texo.knowledge.triangulate.v1",
    reads_events = ["evt.e00b", "evt.e00c", "evt.e00d", "evt.e00e"],
    queries_projections = ["texo.workspace.view.v2", "texo.evidence.view.v1"]
)]
#[tracing::instrument(skip_all)]
fn knowledge_triangulate(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.knowledge.triangulate", || {
        let input: KnowledgeTriangulateInput = parse_input("texo.knowledge.triangulate", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.knowledge.triangulate", error))?;
        let (view, snapshot) = assemble_snapshot_view(input.snapshot.as_deref())?;
        triangulate_from_view(&view, &snapshot, input.target)
    })
}

#[syncbat::operation(
    descriptor = CLAIM_SUPERSEDE,
    register = register_claim_supersede,
    register_item = claim_supersede_item,
    name = "texo.claim.supersede",
    effect = Persist,
    input_schema = "texo.claim.supersede.input.v2",
    output_schema = "texo.claim.supersede.output.v2",
    receipt_kind = "receipt.texo.claim.supersede.v2",
    appends_events = ["evt.e003"],
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn claim_supersede(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.claim.supersede", || {
        let input: ClaimSupersedeInput = parse_input("texo.claim.supersede", input)?;
        if input.old == input.new {
            return Err(TexoError::OpInput {
                op: "texo.claim.supersede".to_string(),
                detail: "old and new claims must differ".to_string(),
            });
        }
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.claim.supersede", error))?;

        let old_entity = entity_for_claim(&input.old);
        let new_entity = entity_for_claim(&input.new);
        let (old_card, new_card, workspace_id) = env::with(|op_env| {
            let old_card = op_env
                .store
                .project::<ClaimCard>(&old_entity, &Freshness::Consistent)?;
            let new_card = op_env
                .store
                .project::<ClaimCard>(&new_entity, &Freshness::Consistent)?;
            Ok::<_, TexoError>((old_card, new_card, op_env.workspace_id.clone()))
        })??;
        let old_card = old_card.ok_or_else(|| TexoError::MissingEntity {
            entity: old_entity.clone(),
        })?;
        let _new_card = new_card.ok_or_else(|| TexoError::MissingEntity {
            entity: new_entity.clone(),
        })?;
        if old_card.phase == 2 && old_card.superseded_by.as_deref() == Some(input.new.as_str()) {
            return Ok(ClaimSupersedeOutput {
                old: input.old,
                new: input.new,
                already_applied: true,
                receipt: None,
            });
        }
        if old_card.phase != 1 {
            return Err(TexoError::Transition {
                machine: CLAIM_MACHINE.to_string(),
                from: old_card.phase,
                to: 2,
                context: Some(format!(
                    "claim {} is already {}{}",
                    input.old,
                    claim_phase_name(old_card.phase),
                    old_card
                        .superseded_by
                        .as_deref()
                        .map_or_else(String::new, |successor| format!(" by {successor}"))
                )),
            });
        }

        let payload = ClaimSupersededV2 {
            old_claim_id: input.old.clone(),
            new_claim_id: input.new.clone(),
            workspace_id,
            reason: input.reason,
            decided_by: input.decided_by,
            observed_at_ms: input.observed_at_ms,
            transition: transition_record(
                CLAIM_MACHINE,
                &old_entity,
                1,
                2,
                vec![TransitionCauseV1 {
                    lane: 0,
                    key: format!("claim:{}", input.new),
                }],
                input.observed_at_ms,
            ),
        };
        append_json(
            "texo.claim.supersede",
            cx,
            <ClaimSupersededV2 as EventPayload>::KIND,
            &payload,
        )?;
        Ok(ClaimSupersedeOutput {
            old: input.old,
            new: input.new,
            already_applied: false,
            receipt: Some(take_one_receipt("texo.claim.supersede")?),
        })
    })
}

#[syncbat::operation(
    descriptor = VERIFY_RUN,
    register = register_verify_run,
    register_item = verify_run_item,
    name = "texo.verify.run",
    effect = Inspect,
    input_schema = "texo.verify.run.input.v2",
    output_schema = "texo.verify.run.output.v2",
    receipt_kind = "receipt.texo.verify.run.v2",
    reads_events = ["evt.e001", "evt.e002", "evt.e003", "evt.e004", "evt.e005", "evt.e006", "evt.e007", "evt.e008", "evt.e009", "evt.e00a", "evt.e00b", "evt.e00c", "evt.e00d", "evt.e00e", "evt.e00f", "evt.e010"],
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
            let view = assemble(&op_env.store, &op_env.workspace_id, &mut cache)?;
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

#[syncbat::operation(
    descriptor = STALENESS_CHECK,
    register = register_staleness_check,
    register_item = staleness_check_item,
    name = "texo.staleness.check",
    effect = Inspect,
    input_schema = "texo.staleness.check.input.v3",
    output_schema = "texo.staleness.check.output.v3",
    receipt_kind = "receipt.texo.staleness.check.v3",
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn staleness_check(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.staleness.check", || {
        let input: StalenessCheckInput = parse_input("texo.staleness.check", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.staleness.check", error))?;
        let (view, snapshot) = assemble_snapshot_view(input.snapshot.as_deref())?;
        let (root, workspace_id) =
            env::with(|op_env| (op_env.root.clone(), op_env.workspace_id.clone()))?;
        let path = resolve_path(&root, &input.path);
        check_staleness_from_view(&view, &workspace_id, &root, &path, snapshot)
    })
}

#[syncbat::operation(
    descriptor = CONTEXT_AGENT,
    register = register_context_agent,
    register_item = context_agent_item,
    name = "texo.context.agent",
    effect = Inspect,
    input_schema = "texo.context.agent.input.v3",
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
        if input.strict_settlement {
            require_complete_settlement()?;
        }
        let (view, snapshot) = assemble_snapshot_view(input.snapshot.as_deref())?;
        build_agent_context_from_view(
            &view,
            input.subject.as_deref(),
            input.include_stale,
            snapshot,
        )
    })
}

#[syncbat::operation(
    descriptor = COMPILE_RUN,
    register = register_compile_run,
    register_item = compile_run_item,
    name = "texo.compile.run",
    effect = Persist,
    input_schema = "texo.compile.run.input.v2",
    output_schema = "texo.compile.run.output.v2",
    receipt_kind = "receipt.texo.compile.run.v2",
    appends_events = ["evt.e005"],
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn compile_run(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.compile.run", || {
        let input: CompileRunInput = parse_input("texo.compile.run", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.compile.run", error))?;
        if input.strict_settlement {
            require_complete_settlement()?;
        }
        let view = assemble_current_view()?;
        let snapshot = snapshot_for_view(&view)?;
        let context = build_agent_context_from_view(&view, None, true, snapshot.clone())?;
        let conflict_report = heuristic::detect_conflicts(&view)?;
        let (root, workspace_id) =
            env::with(|op_env| (op_env.root.clone(), op_env.workspace_id.clone()))?;
        let out_dir = resolve_path(&root, &input.out_dir);
        let stale_report = StalenessReport {
            workspace_id: workspace_id.clone(),
            checked_path: ".".to_string(),
            replayed_through_sequence: view.frontier,
            diagnostics: Vec::new(),
            snapshot,
        };
        let files = compile_artifacts(&context, &view, &stale_report, &conflict_report)?;
        for file in &files {
            let path = out_dir.join(&file.name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, file.contents.as_bytes())?;
        }
        let source_claim_ids = view
            .claims
            .iter()
            .map(|claim| claim.card.claim_id.clone())
            .collect::<Vec<_>>();
        let doc_id = format!(
            "doc_{}",
            &blake3::hash(out_dir.to_string_lossy().as_bytes()).to_hex()[..12]
        );
        append_json(
            "texo.compile.run",
            cx,
            <OnboardingCompiledV2 as EventPayload>::KIND,
            &OnboardingCompiledV2 {
                doc_id,
                workspace_id,
                output_path: input.out_dir.to_string_lossy().to_string(),
                source_claim_ids,
                replayed_through_sequence: view.frontier,
                compiled_at_ms: input.observed_at_ms,
            },
        )?;
        Ok(CompileRunOutput {
            files: files.into_iter().map(|file| file.name).collect::<Vec<_>>(),
            receipt: take_one_receipt("texo.compile.run")?,
        })
    })
}

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
fn conflicts_list(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
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
fn conflicts_commit(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
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
fn conflict_resolve(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
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
            op_env
                .store
                .project::<ConflictCard>(&entity, &Freshness::Consistent)
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

#[syncbat::operation(
    descriptor = RELATE_RUN,
    register = register_relate_run,
    register_item = relate_run_item,
    name = "texo.relate.run",
    effect = Persist,
    input_schema = "texo.relate.run.input.v2",
    output_schema = "texo.relate.run.output.v2",
    receipt_kind = "receipt.texo.relate.run.v2",
    appends_events = ["evt.e003", "evt.e004", "evt.e009", "evt.e00a"],
    queries_projections = ["texo.workspace.view.v2"],
    requires_capabilities = ["texo.cap.model"]
)]
#[tracing::instrument(skip_all)]
fn relate_run(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.relate.run", || {
        let input: RelateRunInput = parse_input("texo.relate.run", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.relate.run", error))?;
        run_relate_pass("texo.relate.run", cx, input.observed_at_ms, input.strict)
    })
}

#[syncbat::operation(
    descriptor = HOST_FINGERPRINT,
    register = register_host_fingerprint,
    register_item = host_fingerprint_item,
    name = "texo.host.fingerprint",
    effect = Inspect,
    input_schema = "texo.host.fingerprint.input.v2",
    output_schema = "texo.host.fingerprint.output.v2",
    receipt_kind = "receipt.texo.host.fingerprint.v2"
)]
#[tracing::instrument(skip_all)]
fn host_fingerprint(input: &[u8], _cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.host.fingerprint", || {
        let _input: HostFingerprintInput = parse_input("texo.host.fingerprint", input)?;
        // TODO(batpak-0.10): replace with hostbat HostModule manifest + fingerprints
        //  once texo bumps past the 0.9.0 HostBuilder gap (freebatteryfactory/batpak#166,
        //  fixed in #169/0.10.0). This hand-rolled digest is content-addressed over the
        //  same declared surface and upgrades in place.
        Ok(crate::host::fingerprint::canonical_interface(
            &crate::ops::catalog(),
        ))
    })
}

#[syncbat::operation(
    descriptor = KNOWLEDGE_INDEX,
    register = register_knowledge_index,
    register_item = knowledge_index_item,
    name = "texo.knowledge.index",
    effect = Persist,
    input_schema = "texo.knowledge.index.input.v1",
    output_schema = "texo.knowledge.index.output.v1",
    receipt_kind = "receipt.texo.knowledge.index.v1",
    appends_events = ["evt.e00b", "evt.e00c", "evt.e00d", "evt.e00f"],
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn knowledge_index(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.knowledge.index", || {
        let input: KnowledgeIndexInput = parse_input("texo.knowledge.index", input)?;
        let limits = input.validated_limits()?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.knowledge.index", error))?;
        let view = assemble_current_view()?;
        let (root, workspace_id) =
            env::with(|op_env| (op_env.root.clone(), op_env.workspace_id.clone()))?;
        let workspace = WorkspaceId::new(workspace_id.clone())?;
        let previous_snapshots = source_snapshots_through(Some(view.frontier))?;
        let repository_id = previous_snapshots.last().cloned().map_or_else(
            || {
                let canonical = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
                RepositoryId::derive(&format!(
                    "texo.repository.v1\u{1f}{workspace_id}\u{1f}{}",
                    canonical.display()
                ))
            },
            |snapshot| snapshot.repository_id,
        );
        let mut capture = capture_git(&root, repository_id, limits)?;
        if latest_source_snapshot(Some(view.frontier))?
            .is_some_and(|existing| existing.snapshot_id == capture.snapshot_id)
        {
            return Ok(KnowledgeIndexOutput {
                workspace_id,
                snapshot_id: capture.snapshot_id,
                base_commit: capture.base_commit,
                dirty: capture.dirty,
                sources_captured: capture.sources.len(),
                evidence_recorded: 0,
                claims_linked: 0,
                relations_recorded: 0,
                already_indexed: true,
                coverage: capture.coverage,
                receipts: Vec::new(),
            });
        }

        let planned = plan_claim_evidence(
            &view,
            &capture.sources,
            &capture.snapshot_id,
            input.observed_at_ms,
        )?;
        for gap in planned.gaps {
            if capture.coverage.gaps.len() < 256 {
                capture.coverage.gaps.push(gap);
            } else {
                capture.coverage.truncated = true;
            }
        }
        if !planned.rows.is_empty() {
            capture.coverage.analysis_quality = AnalysisQuality::Syntactic;
        }
        capture.coverage.occurrences = u64::try_from(planned.rows.len()).unwrap_or(u64::MAX);
        let relations = plan_and_attach_snapshot_relations(
            &root,
            &workspace,
            &mut capture,
            &previous_snapshots,
            input.observed_at_ms,
        )?;
        let snapshot = SourceSnapshotRecordedV1 {
            workspace_id: workspace.clone(),
            repository_id: capture.repository_id,
            snapshot_id: capture.snapshot_id.clone(),
            base_commit: capture.base_commit.clone(),
            base_tree: capture.base_tree,
            index_digest_hex: capture.index_digest_hex,
            overlay_digest_hex: capture.overlay_digest_hex,
            dirty: capture.dirty,
            coverage: capture.coverage.clone(),
            observed_at_ms: input.observed_at_ms,
        };
        let receipts = append_knowledge_plan(
            cx,
            &workspace,
            &snapshot,
            &planned.rows,
            &relations,
            input.observed_at_ms,
        )?;
        Ok(KnowledgeIndexOutput {
            workspace_id,
            snapshot_id: capture.snapshot_id,
            base_commit: capture.base_commit,
            dirty: capture.dirty,
            sources_captured: capture.sources.len(),
            evidence_recorded: planned.rows.len(),
            claims_linked: planned.rows.len(),
            relations_recorded: relations.len(),
            already_indexed: false,
            coverage: capture.coverage,
            receipts,
        })
    })
}

#[syncbat::operation(
    descriptor = CODE_INDEX_BUILD,
    register = register_code_index_build,
    register_item = code_index_build_item,
    name = "texo.code.index.build",
    effect = Persist,
    input_schema = "texo.code.index.build.input.v1",
    output_schema = "texo.code.index.build.output.v1",
    receipt_kind = "receipt.texo.code.index.build.v1",
    appends_events = ["evt.e00e"],
    reads_events = ["evt.e00b", "evt.e00e"],
    queries_projections = ["texo.code.index.v1"]
)]
#[tracing::instrument(skip_all)]
fn code_index_build(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.code.index.build", || {
        let input: CodeIndexBuildInput = parse_input("texo.code.index.build", input)?;
        let limits = input.validated_limits()?;
        let (root, workspace_id) =
            env::with(|op_env| (op_env.root.clone(), op_env.workspace_id.clone()))?;
        let recorded = latest_source_snapshot(None)?.ok_or_else(|| TexoError::Snapshot {
            kind: SnapshotFailureKind::SourceUnavailable,
            detail: "run `texo index` to record a Git source snapshot first".to_string(),
        })?;
        if input
            .snapshot_id
            .as_ref()
            .is_some_and(|wanted| wanted != &recorded.snapshot_id)
        {
            return Err(TexoError::Snapshot {
                kind: SnapshotFailureKind::SourceUnavailable,
                detail: "requested source snapshot is not the latest indexed snapshot".to_string(),
            });
        }
        let capture = capture_git(&root, recorded.repository_id, CaptureLimits::default())?;
        if capture.snapshot_id != recorded.snapshot_id {
            return Err(TexoError::Snapshot {
                kind: SnapshotFailureKind::SourceUnavailable,
                detail: "Git commit/index/worktree changed; run source indexing again before code indexing".to_string(),
            });
        }
        let scip_bytes = input
            .scip_path
            .as_deref()
            .map(|path| read_scip(&root, path, limits.max_scip_bytes))
            .transpose()?;
        let prepared = build_code_index(&capture, scip_bytes.as_deref(), limits)?;
        let artifact_path = persist_code_index(&root, &prepared)?;
        let payload = CodeIndexRecordedV1 {
            workspace_id: WorkspaceId::new(workspace_id.clone())?,
            snapshot_id: recorded.snapshot_id,
            index_id: prepared.artifact.index_id.clone(),
            format: prepared.artifact.format,
            analyzer_fingerprint: prepared.artifact.analyzer_fingerprint.clone(),
            artifact_digest_hex: prepared.artifact_digest_hex.clone(),
            coverage: prepared.artifact.coverage.clone(),
            observed_at_ms: input.observed_at_ms,
        };
        append_json(
            "texo.code.index.build",
            cx,
            <CodeIndexRecordedV1 as EventPayload>::KIND,
            &payload,
        )?;
        let relative_artifact = artifact_path
            .strip_prefix(&root)
            .unwrap_or(&artifact_path)
            .to_string_lossy()
            .to_string();
        Ok(CodeIndexBuildOutput {
            workspace_id,
            snapshot_id: payload.snapshot_id,
            index_id: payload.index_id,
            format: payload.format,
            analyzer_fingerprint: payload.analyzer_fingerprint,
            artifact_digest_hex: payload.artifact_digest_hex,
            artifact_path: relative_artifact,
            coverage: payload.coverage,
            receipt: take_one_receipt("texo.code.index.build")?,
        })
    })
}

#[syncbat::operation(
    descriptor = KNOWLEDGE_RECONCILE,
    register = register_knowledge_reconcile,
    register_item = knowledge_reconcile_item,
    name = "texo.knowledge.reconcile",
    effect = Persist,
    input_schema = "texo.knowledge.reconcile.input.v1",
    output_schema = "texo.knowledge.reconcile.output.v1",
    receipt_kind = "receipt.texo.knowledge.reconcile.v1",
    appends_events = ["evt.e00c", "evt.e00d", "evt.e010"],
    reads_events = ["evt.e002", "evt.e00b", "evt.e00c", "evt.e00d", "evt.e00e"],
    queries_projections = ["texo.workspace.view.v2", "texo.code.index.v1"],
    requires_capabilities = ["texo.cap.model"]
)]
#[tracing::instrument(skip_all)]
fn knowledge_reconcile(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.knowledge.reconcile", || {
        let input: KnowledgeReconcileInput = parse_input("texo.knowledge.reconcile", input)?;
        let (limits, budget_secs, concurrency) = input.validated()?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.knowledge.reconcile", error))?;
        let view = assemble_current_view()?;
        let snapshot =
            latest_source_snapshot(Some(view.frontier))?.ok_or_else(|| TexoError::OpInput {
                op: "texo.knowledge.reconcile".to_string(),
                detail: "no source snapshot exists; run `texo index` first".to_string(),
            })?;
        let loaded = load_code_artifact_at(view.frontier, &snapshot.snapshot_id)?;
        let artifact = loaded.artifact.ok_or_else(|| TexoError::OpInput {
            op: "texo.knowledge.reconcile".to_string(),
            detail: "the current source snapshot has no available code index; run `texo index`"
                .to_string(),
        })?;
        let claims = reconcile_claims(&view)?;
        let plan = plan_candidates(&claims, &artifact, limits);
        let candidates_considered = plan.candidates.len();
        let evidence = evidence_projection_through(view.frontier)?;
        let mut already_linked = 0;
        let candidates = plan
            .candidates
            .into_iter()
            .filter(|candidate| {
                let linked = evidence
                    .for_claim(candidate.claim_id.as_str())
                    .iter()
                    .any(|item| {
                        item.occurrence.occurrence_id == candidate.occurrence.occurrence_id
                    });
                already_linked += usize::from(linked);
                !linked
            })
            .collect::<Vec<_>>();
        let (root, gateway) =
            env::with(|op_env| (op_env.root.clone(), op_env.config.gateway.clone()))?;
        let backend = evaluate_with_backends(
            &root,
            gateway.as_ref(),
            &candidates,
            std::time::Duration::from_secs(budget_secs),
            concurrency,
        )?;
        let workspace_id = WorkspaceId::new(view.workspace_id.clone())?;
        let ReconcileBackendOutput {
            proposals,
            unresolved: backend_unresolved,
            judge_fingerprint,
        } = backend;
        let (accepted, rejected) = append_reconciliation_proposals(
            cx,
            &workspace_id,
            input.observed_at_ms,
            limits.min_score_ppm,
            &judge_fingerprint,
            proposals,
        )?;
        let unresolved = backend_unresolved
            .iter()
            .map(reconcile_unresolved_row)
            .collect::<Vec<_>>();
        let mut coverage = artifact.coverage;
        if plan.truncated {
            coverage.truncated = true;
            if !coverage
                .gaps
                .iter()
                .any(|gap| gap.kind == CoverageGapKind::BudgetExceeded)
            {
                coverage.gaps.push(CoverageGap {
                    path: None,
                    kind: CoverageGapKind::BudgetExceeded,
                });
            }
        }
        let partial = coverage.truncated || !coverage.gaps.is_empty() || !unresolved.is_empty();
        Ok(KnowledgeReconcileOutput {
            outcome: if partial {
                ReconcileCompletion::Partial
            } else {
                ReconcileCompletion::Complete
            },
            snapshot_id: snapshot.snapshot_id,
            candidates_considered,
            already_linked,
            accepted,
            rejected,
            unresolved,
            coverage,
            receipts: take_receipts()?,
        })
    })
}

#[syncbat::operation(
    descriptor = STATS_READ,
    register = register_stats_read,
    register_item = stats_read_item,
    name = "texo.stats.read",
    effect = Inspect,
    input_schema = "texo.stats.read.input.v1",
    output_schema = "texo.stats.read.output.v1",
    receipt_kind = "receipt.texo.stats.read.v1",
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn stats_read(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.stats.read", || {
        let _input: StatsReadInput = parse_input("texo.stats.read", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.stats.read", error))?;
        let view = assemble_current_view()?;
        let (root, config) = env::with(|op_env| (op_env.root.clone(), op_env.config.clone()))?;
        let store_path = config.store_path_buf(&root);
        let projection_path = root
            .join(".texo/cache/workspace-view")
            .join(format!("{}.bin", view.workspace_id));
        let context = build_agent_context_from_view(&view, None, true, snapshot_for_view(&view)?)?;
        let agent_context_bytes = serde_json::to_vec(&context)?.len();
        Ok(StatsReadOutput {
            claims_total: view.claims.len(),
            events_total: workspace_event_count()?,
            journal_bytes: journal_file_bytes(&store_path)?,
            projection_bytes: file_bytes(&projection_path)?,
            agent_context_bytes: u64::try_from(agent_context_bytes).unwrap_or(u64::MAX),
            frontier_sequence: view.frontier,
        })
    })
}

#[syncbat::operation(
    descriptor = WORKSPACE_STATUS,
    register = register_workspace_status,
    register_item = workspace_status_item,
    name = "texo.workspace.status",
    effect = Inspect,
    input_schema = "texo.workspace.status.input.v2",
    output_schema = "texo.workspace.status.output.v2",
    receipt_kind = "receipt.texo.workspace.status.v2",
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn workspace_status(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.workspace.status", || {
        let input: WorkspaceStatusInput = parse_input("texo.workspace.status", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.workspace.status", error))?;
        let (view, snapshot) = assemble_snapshot_view(input.snapshot.as_deref())?;
        let settlement = authoritative_settlements(Some(view.frontier))?;
        let unresolved_pairs = settlement.unresolved_pairs;
        let (coverage, code_index_available) = status_coverage(&view, &snapshot)?;
        Ok(WorkspaceStatusOutput {
            workspace_id: view.workspace_id.clone(),
            frontier: view.frontier,
            freshness: view.freshness,
            claims_total: view.claims.len(),
            open_conflicts: view.conflicts.iter().filter(|card| card.phase == 1).count(),
            settlement_complete: unresolved_pairs == 0,
            unresolved_pairs,
            authority_warnings: settlement.warnings.len(),
            code_index_available,
            coverage,
            snapshot,
        })
    })
}

const DOMAIN_KINDS: [EventKind; 16] = [
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
];

/// Return the operation registration items.
#[must_use]
pub fn catalog() -> Vec<OperationRegisterItem> {
    vec![
        workspace_init_item(),
        ingest_run_item(),
        claims_list_item(),
        claims_search_item(),
        knowledge_search_item(),
        claim_explain_item(),
        claim_supersede_item(),
        verify_run_item(),
        staleness_check_item(),
        context_agent_item(),
        compile_run_item(),
        conflicts_list_item(),
        conflicts_commit_item(),
        conflict_resolve_item(),
        relate_run_item(),
        host_fingerprint_item(),
        stats_read_item(),
        knowledge_index_item(),
        code_index_build_item(),
        knowledge_reconcile_item(),
        knowledge_triangulate_item(),
        workspace_status_item(),
    ]
}

/// Register all built-in texo operations.
///
/// # Errors
/// Returns [`syncbat::BuildError`] if a descriptor or handler cannot be
/// registered with the builder.
pub fn register_all(builder: &mut CoreBuilder) -> Result<(), syncbat::BuildError> {
    register_workspace_init(builder)?;
    register_ingest_run(builder)?;
    register_claims_list(builder)?;
    register_claims_search(builder)?;
    register_knowledge_search(builder)?;
    register_claim_explain(builder)?;
    register_claim_supersede(builder)?;
    register_verify_run(builder)?;
    register_staleness_check(builder)?;
    register_context_agent(builder)?;
    register_compile_run(builder)?;
    register_conflicts_list(builder)?;
    register_conflicts_commit(builder)?;
    register_conflict_resolve(builder)?;
    register_relate_run(builder)?;
    register_host_fingerprint(builder)?;
    register_stats_read(builder)?;
    register_knowledge_index(builder)?;
    register_code_index_build(builder)?;
    register_knowledge_reconcile(builder)?;
    register_knowledge_triangulate(builder)?;
    register_workspace_status(builder)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct WorkspaceInitInput {
    workspace_id: String,
}

#[derive(Debug, Deserialize)]
struct KnowledgeIndexInput {
    observed_at_ms: u64,
    #[serde(default)]
    max_files: Option<usize>,
    #[serde(default)]
    max_file_bytes: Option<u64>,
    #[serde(default)]
    max_total_bytes: Option<u64>,
}

impl KnowledgeIndexInput {
    fn validated_limits(&self) -> Result<CaptureLimits, TexoError> {
        let defaults = CaptureLimits::default();
        let limits = CaptureLimits {
            max_files: self.max_files.unwrap_or(defaults.max_files),
            max_file_bytes: self.max_file_bytes.unwrap_or(defaults.max_file_bytes),
            max_total_bytes: self.max_total_bytes.unwrap_or(defaults.max_total_bytes),
        };
        if limits.max_files == 0
            || limits.max_files > 100_000
            || limits.max_file_bytes == 0
            || limits.max_file_bytes > 16 * 1024 * 1024
            || limits.max_total_bytes == 0
            || limits.max_total_bytes > 512 * 1024 * 1024
        {
            return Err(TexoError::OpInput {
                op: "texo.knowledge.index".to_string(),
                detail: "capture limits must be non-zero and at most 100000 files, 16 MiB per file, and 512 MiB total".to_string(),
            });
        }
        Ok(limits)
    }
}

#[derive(Debug, Serialize)]
struct KnowledgeIndexOutput {
    workspace_id: String,
    snapshot_id: crate::knowledge::SourceSnapshotId,
    base_commit: crate::knowledge::GitObjectId,
    dirty: bool,
    sources_captured: usize,
    evidence_recorded: usize,
    claims_linked: usize,
    relations_recorded: usize,
    already_indexed: bool,
    coverage: KnowledgeCoverage,
    receipts: Vec<ReceiptNote>,
}

#[derive(Debug, Deserialize)]
struct CodeIndexBuildInput {
    #[serde(default)]
    snapshot_id: Option<crate::knowledge::SourceSnapshotId>,
    #[serde(default)]
    scip_path: Option<PathBuf>,
    observed_at_ms: u64,
    #[serde(default)]
    max_scip_bytes: Option<u64>,
    #[serde(default)]
    max_documents: Option<usize>,
    #[serde(default)]
    max_occurrences: Option<usize>,
    #[serde(default)]
    analysis_budget_secs: Option<u64>,
}

impl CodeIndexBuildInput {
    fn validated_limits(&self) -> Result<CodeIndexLimits, TexoError> {
        let defaults = CodeIndexLimits::default();
        let limits = CodeIndexLimits {
            max_scip_bytes: self.max_scip_bytes.unwrap_or(defaults.max_scip_bytes),
            max_documents: self.max_documents.unwrap_or(defaults.max_documents),
            max_occurrences: self.max_occurrences.unwrap_or(defaults.max_occurrences),
            analysis_budget: std::time::Duration::from_secs(
                self.analysis_budget_secs
                    .unwrap_or(defaults.analysis_budget.as_secs()),
            ),
        };
        if limits.max_scip_bytes == 0
            || limits.max_scip_bytes > 256 * 1024 * 1024
            || limits.max_documents == 0
            || limits.max_documents > 100_000
            || limits.max_occurrences == 0
            || limits.max_occurrences > 2_000_000
            || limits.analysis_budget.is_zero()
            || limits.analysis_budget > std::time::Duration::from_secs(300)
        {
            return Err(TexoError::OpInput {
                op: "texo.code.index.build".to_string(),
                detail: "code-index limits must be non-zero and at most 256 MiB, 100000 documents, 2000000 occurrences, and 300 seconds".to_string(),
            });
        }
        Ok(limits)
    }
}

#[derive(Debug, Serialize)]
struct CodeIndexBuildOutput {
    workspace_id: String,
    snapshot_id: crate::knowledge::SourceSnapshotId,
    index_id: CodeIndexId,
    format: crate::knowledge::CodeIndexFormat,
    analyzer_fingerprint: String,
    artifact_digest_hex: String,
    artifact_path: String,
    coverage: KnowledgeCoverage,
    receipt: ReceiptNote,
}

#[derive(Debug, Deserialize)]
struct StatsReadInput {}

#[derive(Debug, Deserialize)]
struct WorkspaceStatusInput {
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
struct StatsReadOutput {
    claims_total: usize,
    events_total: usize,
    journal_bytes: u64,
    projection_bytes: u64,
    agent_context_bytes: u64,
    frontier_sequence: u64,
}

#[derive(Debug, Serialize)]
struct WorkspaceStatusOutput {
    workspace_id: String,
    frontier: u64,
    freshness: crate::claims::workspace::ProjectionFreshness,
    claims_total: usize,
    open_conflicts: usize,
    settlement_complete: bool,
    unresolved_pairs: usize,
    authority_warnings: usize,
    code_index_available: bool,
    snapshot: SnapshotRead,
    coverage: KnowledgeCoverage,
}

#[derive(Debug, Serialize)]
struct WorkspaceInitOutput {
    workspace_id: String,
    config_path: String,
    already_initialized: bool,
    receipt: ReceiptNote,
}

#[derive(Debug, Deserialize)]
struct IngestRunInput {
    path: PathBuf,
    dry_run: bool,
    #[serde(default)]
    strict: bool,
    observed_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum IngestCompletion {
    Complete,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum SourceFailureCode {
    #[serde(rename = "source.utf8")]
    Utf8,
    #[serde(rename = "source.io")]
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SourceSkipRow {
    path: String,
    code: SourceFailureCode,
    detail: String,
}

#[derive(Debug, Serialize)]
struct IngestRunOutput {
    outcome: IngestCompletion,
    workspace_id: String,
    sources_observed: u32,
    claims_recorded: u32,
    claims_superseded: u32,
    dry_run: bool,
    empty: bool,
    skipped: Vec<SourceSkipRow>,
    skipped_total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped_artifact: Option<String>,
    phase_ms: IngestPhaseMs,
    events_appended: u64,
    receipts: Vec<ReceiptNote>,
}

#[derive(Debug, Serialize)]
struct IngestPhaseMs {
    discover: u64,
    extract: u64,
    append: u64,
    project: u64,
}

#[derive(Debug, Deserialize)]
struct ClaimsListInput {
    subject: Option<String>,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaimsSearchInput {
    query: Option<String>,
    subject: Option<String>,
    status: Option<crate::claims::status::ClaimStatus>,
    limit: Option<usize>,
    cursor: Option<String>,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
struct ClaimsListOutput {
    workspace_id: String,
    frontier: u64,
    claims: Vec<AgentClaimRow>,
    snapshot: SnapshotRead,
}

#[derive(Debug, Serialize)]
struct ClaimsSearchOutput {
    workspace_id: String,
    frontier: u64,
    freshness: crate::claims::workspace::ProjectionFreshness,
    total: usize,
    returned: usize,
    has_more: bool,
    next_cursor: Option<String>,
    claims: Vec<AgentClaimRow>,
    snapshot: SnapshotRead,
}

#[derive(Debug, Deserialize)]
struct KnowledgeSearchInput {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    status: Option<crate::claims::status::ClaimStatus>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
struct KnowledgeSearchOutput {
    workspace_id: String,
    frontier: u64,
    total: usize,
    returned: usize,
    has_more: bool,
    next_cursor: Option<String>,
    results: Vec<KnowledgeSearchResult>,
    code_index_available: bool,
    coverage: KnowledgeCoverage,
    snapshot: SnapshotRead,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum KnowledgeSearchResult {
    Claim { claim: AgentClaimRow },
    Code { occurrence: CodeOccurrence },
}

struct RankedKnowledgeResult {
    rank: u8,
    key: String,
    result: KnowledgeSearchResult,
}

#[derive(Debug, Deserialize)]
struct ClaimExplainInput {
    claim_id: String,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
struct ClaimExplainOutput {
    card: ClaimCard,
    timeline: Vec<TimelineEntry>,
    answer_state: AnswerState,
    evidence: Vec<ClaimEvidence>,
    coverage: KnowledgeCoverage,
    snapshot: SnapshotRead,
}

#[derive(Debug, Deserialize)]
struct KnowledgeTriangulateInput {
    target: TriangulationTarget,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
struct KnowledgeTriangulateOutput {
    target: TriangulationTarget,
    answer_state: AnswerState,
    assertions: Vec<AgentClaimRow>,
    evidence: Vec<ClaimEvidence>,
    structural_evidence: Vec<CodeOccurrence>,
    uncertainty: Vec<UncertaintyReason>,
    coverage: KnowledgeCoverage,
    settlement_complete: bool,
    snapshot: SnapshotRead,
}

#[derive(Debug, Deserialize)]
struct ClaimSupersedeInput {
    old: String,
    new: String,
    reason: String,
    decided_by: String,
    observed_at_ms: u64,
}

#[derive(Debug, Serialize)]
struct ClaimSupersedeOutput {
    old: String,
    new: String,
    already_applied: bool,
    receipt: Option<ReceiptNote>,
}

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

#[derive(Debug, Deserialize)]
struct StalenessCheckInput {
    path: PathBuf,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
struct StalenessReport {
    workspace_id: String,
    checked_path: String,
    replayed_through_sequence: u64,
    diagnostics: Vec<StaleDiagnostic>,
    snapshot: SnapshotRead,
}

#[derive(Debug, Serialize)]
struct StaleDiagnostic {
    file: String,
    line_start: u32,
    line_end: u32,
    severity: DiagnosticSeverity,
    message: String,
    claim_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    superseded_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<DiagnosticSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    receipt: Option<AgentReceiptRow>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum DiagnosticSeverity {
    Warning,
}

#[derive(Debug, Serialize)]
struct DiagnosticSource {
    path: String,
    line_start: u32,
}

#[derive(Debug, Deserialize)]
struct ContextAgentInput {
    subject: Option<String>,
    include_stale: bool,
    #[serde(default)]
    strict_settlement: bool,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
struct AgentContextOutput {
    workspace_id: String,
    replayed_through_sequence: u64,
    freshness: FreshnessView,
    claims: Vec<AgentClaimRow>,
    stale_claims: Vec<AgentStaleClaimRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    conflicts: Vec<AgentConflictRow>,
    snapshot: SnapshotRead,
}

#[derive(Debug, Serialize)]
struct FreshnessView {
    kind: crate::claims::workspace::ProjectionFreshness,
    description: String,
}

#[derive(Debug, Serialize)]
struct AgentStaleClaimRow {
    claim_id: String,
    text: String,
    superseded_by: String,
}

#[derive(Debug, Serialize)]
struct AgentConflictRow {
    conflict_id: String,
    claim_a: String,
    claim_a_text: String,
    claim_b: String,
    claim_b_text: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct CompileRunInput {
    out_dir: PathBuf,
    observed_at_ms: u64,
    #[serde(default)]
    strict_settlement: bool,
}

#[derive(Debug, Serialize)]
struct CompileRunOutput {
    files: Vec<String>,
    receipt: ReceiptNote,
}

struct CompileFile {
    name: String,
    contents: String,
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

#[derive(Debug, Deserialize)]
struct RelateRunInput {
    observed_at_ms: u64,
    #[serde(default)]
    strict: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum RelateCompletion {
    Complete,
    Partial,
}

#[derive(Debug, Serialize)]
pub(crate) struct RelateRunOutput {
    outcome: RelateCompletion,
    pub(crate) claims_related: usize,
    pub(crate) supersessions: Vec<RelateSupersessionRow>,
    pub(crate) conflicts: Vec<RelateConflictRow>,
    unresolved: Vec<crate::relate::settlement::UnresolvedPair>,
    held: Vec<crate::relate::settlement::HeldDecision>,
    warnings: Vec<String>,
    authority_warnings: Vec<crate::relate::settlement::AuthorityWarning>,
    pub(crate) receipts: Vec<ReceiptNote>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RelateSupersessionRow {
    old_claim_id: String,
    new_claim_id: String,
    reason: String,
    cache_key: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct RelateConflictRow {
    conflict_id: String,
    claim_a: String,
    claim_b: String,
    reason: String,
    cache_key: String,
}

#[derive(Debug, Deserialize)]
struct HostFingerprintInput {}

#[derive(Debug, Serialize)]
struct AgentClaimRow {
    claim_id: String,
    status: crate::claims::status::ClaimStatus,
    subject_hint: Option<String>,
    text: String,
    source: AgentSourceRow,
    receipt: AgentReceiptRow,
    supersedes: Vec<String>,
    superseded_by: Option<String>,
}

#[derive(Debug, Serialize)]
struct AgentSourceRow {
    source_id: String,
    path: String,
    line_start: u32,
}

#[derive(Debug, Clone, Serialize)]
struct AgentReceiptRow {
    event_id: String,
    sequence: u64,
}

pub(crate) struct PlannedSource {
    pub(crate) observed: SourceObservedV2,
    pub(crate) claims: Vec<ClaimRecordedV2>,
}

pub(crate) struct SourcePlan {
    pub(crate) sources: Vec<PlannedSource>,
    pub(crate) skipped: Vec<SourceSkipRow>,
    empty: bool,
    succeeded: usize,
    discover_ms: u64,
    extract_ms: u64,
}

#[derive(Debug, Deserialize)]
struct CmdClaimLine {
    line_start: u32,
    text: String,
    normalized_text: String,
    subject_hint: Option<String>,
    predicate_hint: Option<String>,
    object_hint: Option<String>,
    confidence_ppm: u32,
    char_start: Option<u32>,
    char_end: Option<u32>,
    extractor_model: Option<String>,
    prompt_version: Option<String>,
}

pub(crate) fn run_op<T: Serialize>(
    op: &'static str,
    f: impl FnOnce() -> Result<T, TexoError>,
) -> HandlerResult {
    let output = f()?;
    serde_json::to_vec(&output).map_err(|error| {
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
    serde_json::from_slice(input).map_err(|error| TexoError::OpInput {
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
    let bytes = serde_json::to_vec(payload)?;
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

fn config_error(error: crate::config::ConfigError) -> TexoError {
    TexoError::Config {
        detail: error.to_string(),
        source: Some(Box::new(error)),
    }
}

pub(crate) fn assemble_current_view() -> Result<std::sync::Arc<WorkspaceView>, TexoError> {
    env::with(|op_env| {
        let mut cache = op_env.cache.borrow_mut();
        assemble(&op_env.store, &op_env.workspace_id, &mut cache)
    })?
}

fn assemble_snapshot_view(
    requested: Option<&str>,
) -> Result<(std::sync::Arc<WorkspaceView>, SnapshotRead), TexoError> {
    let Some(requested) = requested else {
        let view = assemble_current_view()?;
        let snapshot = snapshot_for_view(&view)?;
        return Ok((view, snapshot));
    };
    let (store, workspace) = env::with(|op_env| {
        (
            std::sync::Arc::clone(&op_env.store),
            op_env.workspace_id.clone(),
        )
    })?;
    let workspace_id = WorkspaceId::new(workspace.clone())?;
    let descriptor =
        SnapshotToken::resolve_for_workspace(requested, &workspace_id).map_err(|error| {
            TexoError::Snapshot {
                kind: SnapshotFailureKind::InvalidToken,
                detail: error.to_string(),
            }
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
    let view = assemble_through(&store, &workspace, descriptor.frontier).map_err(|error| {
        TexoError::Snapshot {
            kind: SnapshotFailureKind::Unavailable,
            detail: error.to_string(),
        }
    })?;
    Ok((view, SnapshotRead::new(descriptor)))
}

fn snapshot_for_view(view: &WorkspaceView) -> Result<SnapshotRead, TexoError> {
    let workspace_id = WorkspaceId::new(view.workspace_id.clone())?;
    let anchor_event_id_hex = env::with(|op_env| {
        anchor_at_frontier(&op_env.store, &op_env.workspace_id, view.frontier)
    })??;
    let source_snapshot_id =
        latest_source_snapshot(Some(view.frontier))?.map(|snapshot| snapshot.snapshot_id);
    Ok(SnapshotRead::new(SnapshotDescriptor {
        workspace_id,
        frontier: view.frontier,
        anchor_event_id_hex,
        source_snapshot_id,
    }))
}

fn validate_snapshot_anchor(
    store: &batpak::store::Store<batpak::store::Open>,
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

fn anchor_at_frontier(
    store: &batpak::store::Store<batpak::store::Open>,
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

fn claim_timeline_through(entity: &str, frontier: u64) -> Result<ClaimTimeline, TexoError> {
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

fn coverage_for_view(view: &WorkspaceView, snapshot: &SnapshotRead) -> KnowledgeCoverage {
    if let Ok(Some(recorded)) = latest_source_snapshot(Some(view.frontier)) {
        if Some(&recorded.snapshot_id) == snapshot.descriptor.source_snapshot_id.as_ref() {
            return recorded.coverage;
        }
    }
    KnowledgeCoverage {
        analysis_quality: AnalysisQuality::Unavailable,
        sources_examined: u64::try_from(view.sources.len()).unwrap_or(u64::MAX),
        occurrences: u64::try_from(view.claims.len()).unwrap_or(u64::MAX),
        truncated: false,
        gaps: vec![CoverageGap {
            path: None,
            kind: CoverageGapKind::SourceSnapshotUnavailable,
        }],
    }
}

fn status_coverage(
    view: &WorkspaceView,
    snapshot: &SnapshotRead,
) -> Result<(KnowledgeCoverage, bool), TexoError> {
    let mut coverage = coverage_for_view(view, snapshot);
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

fn latest_source_snapshot(
    frontier: Option<u64>,
) -> Result<Option<SourceSnapshotRecordedV1>, TexoError> {
    Ok(source_snapshots_through(frontier)?.pop())
}

fn source_snapshots_through(
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

fn plan_snapshot_relations(
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

fn overlay_aware_comparison(
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

fn plan_and_attach_snapshot_relations(
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

fn evidence_projection_through(frontier: u64) -> Result<EvidenceProjection, TexoError> {
    env::with(|op_env| assemble_evidence_through(&op_env.store, &op_env.workspace_id, frontier))?
}

fn temporal_projection_through(frontier: u64) -> Result<TemporalProjection, TexoError> {
    env::with(|op_env| assemble_temporal_through(&op_env.store, &op_env.workspace_id, frontier))?
}

fn semantic_temporal_policy(
    view: &WorkspaceView,
    claims: &[(ClaimId, SemanticClaimView)],
) -> Result<RelateTemporalPolicy, TexoError> {
    let evidence = evidence_projection_through(view.frontier)?;
    let relations = temporal_projection_through(view.frontier)?;
    let mut policy = RelateTemporalPolicy::default();
    for (claim_id, _) in claims {
        if let Some(latest) = evidence
            .for_claim(claim_id.as_str())
            .iter()
            .filter(|item| {
                item.method == EvidenceLinkMethod::Deterministic
                    && item.stance == EvidenceStance::Supports
            })
            .max_by_key(|item| item.link_sequence)
        {
            policy.bind_claim(claim_id, &latest.occurrence.snapshot_id);
        }
    }
    for (left, right, relation) in relations.facts() {
        policy.insert_relation_ids(left, right, relation);
    }
    Ok(policy)
}

fn triangulate_from_view(
    view: &WorkspaceView,
    snapshot: &SnapshotRead,
    target: TriangulationTarget,
) -> Result<KnowledgeTriangulateOutput, TexoError> {
    validate_triangulation_target(&target)?;
    let projection = evidence_projection_through(view.frontier)?;
    let claim_ids = triangulation_claim_ids(view, &target)?;
    let assertions = claim_list_rows(view, None)?
        .into_iter()
        .filter(|claim| claim_ids.contains(&claim.claim_id))
        .collect::<Vec<_>>();
    let mut evidence = claim_ids
        .iter()
        .flat_map(|claim_id| projection.for_claim(claim_id).iter().cloned())
        .collect::<Vec<_>>();
    evidence.retain(|item| evidence_matches_target(item, &target));
    let mut coverage = coverage_for_view(view, snapshot);
    let code = code_evidence_for_target(view.frontier, snapshot, &target)?;
    if let Some(code_coverage) = &code.coverage {
        merge_coverage(&mut coverage, code_coverage);
    }
    if projection.is_incomplete() {
        coverage.gaps.push(CoverageGap {
            path: None,
            kind: CoverageGapKind::AnalysisIncomplete,
        });
    }
    let settlement = authoritative_settlements(Some(view.frontier))?;
    let settlement_complete = settlement.unresolved_pairs == 0;
    let mut uncertainty = BTreeSet::new();
    if snapshot.descriptor.source_snapshot_id.is_none() {
        uncertainty.insert(UncertaintyReason::SourceSnapshotUnavailable);
    }
    if coverage.truncated || !coverage.gaps.is_empty() {
        uncertainty.insert(UncertaintyReason::PartialCoverage);
    }
    if !settlement_complete {
        uncertainty.insert(UncertaintyReason::SettlementIncomplete);
    }
    if matches!(target, TriangulationTarget::Symbol { .. }) && code.unavailable {
        uncertainty.insert(UncertaintyReason::CodeIndexUnavailable);
        if !coverage
            .gaps
            .iter()
            .any(|gap| gap.kind == CoverageGapKind::CodeIndexUnavailable)
        {
            coverage.gaps.push(CoverageGap {
                path: None,
                kind: CoverageGapKind::CodeIndexUnavailable,
            });
        }
    }
    if !assertions.is_empty() && evidence.is_empty() {
        uncertainty.insert(UncertaintyReason::ExactEvidenceUnavailable);
    }
    let answer_state = answer_state_for_rows(&assertions, &evidence, &code.rows);
    Ok(KnowledgeTriangulateOutput {
        target,
        answer_state,
        assertions,
        evidence,
        structural_evidence: code.rows,
        uncertainty: uncertainty.into_iter().collect(),
        coverage,
        settlement_complete,
        snapshot: snapshot.clone(),
    })
}

#[derive(Default)]
struct CodeEvidenceLookup {
    rows: Vec<CodeOccurrence>,
    coverage: Option<KnowledgeCoverage>,
    unavailable: bool,
}

#[derive(Default)]
struct LoadedCodeArtifact {
    artifact: Option<CodeIndexArtifact>,
    coverage: Option<KnowledgeCoverage>,
    unavailable: bool,
}

fn code_evidence_for_target(
    frontier: u64,
    snapshot: &SnapshotRead,
    target: &TriangulationTarget,
) -> Result<CodeEvidenceLookup, TexoError> {
    let Some(source_snapshot_id) = snapshot.descriptor.source_snapshot_id.as_ref() else {
        return Ok(CodeEvidenceLookup {
            unavailable: true,
            ..CodeEvidenceLookup::default()
        });
    };
    let loaded = load_code_artifact_at(frontier, source_snapshot_id)?;
    let Some(artifact) = loaded.artifact else {
        return Ok(CodeEvidenceLookup {
            coverage: loaded.coverage,
            unavailable: loaded.unavailable,
            ..CodeEvidenceLookup::default()
        });
    };
    let mut rows = artifact
        .occurrences
        .into_iter()
        .filter(|occurrence| code_occurrence_matches(occurrence, target))
        .take(MAX_TRIANGULATION_CODE_OCCURRENCES + 1)
        .collect::<Vec<_>>();
    let mut coverage = artifact.coverage;
    if rows.len() > MAX_TRIANGULATION_CODE_OCCURRENCES {
        rows.truncate(MAX_TRIANGULATION_CODE_OCCURRENCES);
        coverage.truncated = true;
        if !coverage
            .gaps
            .iter()
            .any(|gap| gap.path.is_none() && gap.kind == CoverageGapKind::BudgetExceeded)
        {
            coverage.gaps.push(CoverageGap {
                path: None,
                kind: CoverageGapKind::BudgetExceeded,
            });
        }
    }
    Ok(CodeEvidenceLookup {
        rows,
        coverage: Some(coverage),
        unavailable: false,
    })
}

fn load_code_artifact_at(
    frontier: u64,
    source_snapshot_id: &crate::knowledge::SourceSnapshotId,
) -> Result<LoadedCodeArtifact, TexoError> {
    let Some(recorded) = latest_code_index(Some(frontier), source_snapshot_id)? else {
        return Ok(LoadedCodeArtifact {
            unavailable: true,
            ..LoadedCodeArtifact::default()
        });
    };
    let artifact = env::with(|op_env| {
        load_code_index(
            &op_env.root,
            &recorded.index_id,
            &recorded.artifact_digest_hex,
        )
    })??;
    if artifact
        .as_ref()
        .is_some_and(|artifact| artifact.snapshot_id != *source_snapshot_id)
    {
        return Err(TexoError::Snapshot {
            kind: SnapshotFailureKind::SourceUnavailable,
            detail: "code-index artifact belongs to a different source snapshot".to_string(),
        });
    }
    Ok(LoadedCodeArtifact {
        unavailable: artifact.is_none(),
        coverage: Some(recorded.coverage),
        artifact,
    })
}

fn latest_code_index(
    frontier: Option<u64>,
    snapshot_id: &crate::knowledge::SourceSnapshotId,
) -> Result<Option<CodeIndexRecordedV1>, TexoError> {
    env::with(|op_env| {
        let region = Region::scope(scope_for_workspace(&op_env.workspace_id));
        let mut after = None;
        let mut latest = None;
        'pages: loop {
            let page = op_env.store.query_entries_after(&region, after, 256);
            if page.is_empty() {
                break;
            }
            for entry in &page {
                if frontier.is_some_and(|frontier| entry.global_sequence() > frontier) {
                    break 'pages;
                }
                if entry.event_kind() == <CodeIndexRecordedV1 as EventPayload>::KIND {
                    let raw = op_env.store.read_raw(entry.event_id())?;
                    let payload =
                        batpak::encoding::from_bytes::<CodeIndexRecordedV1>(&raw.event.payload)
                            .map_err(|error| TexoError::Decode {
                                entity: entry.coord().entity().to_string(),
                                detail: error.to_string(),
                            })?;
                    if payload.snapshot_id == *snapshot_id {
                        latest = Some(payload);
                    }
                }
            }
            after = page.last().map(batpak::store::IndexEntry::global_sequence);
        }
        Ok::<_, TexoError>(latest)
    })?
}

fn code_occurrence_matches(occurrence: &CodeOccurrence, target: &TriangulationTarget) -> bool {
    match target {
        TriangulationTarget::Claim { .. } => false,
        TriangulationTarget::Path {
            path,
            line_start,
            line_end,
        } => {
            occurrence.path == *path
                && line_start.is_none_or(|start| occurrence.line_range.end >= start)
                && line_end.is_none_or(|end| occurrence.line_range.start <= end)
        }
        TriangulationTarget::Symbol { symbol } => {
            occurrence.symbol == *symbol || occurrence.display_name == *symbol
        }
    }
}

fn merge_coverage(target: &mut KnowledgeCoverage, code: &KnowledgeCoverage) {
    if analysis_quality_rank(code.analysis_quality) > analysis_quality_rank(target.analysis_quality)
    {
        target.analysis_quality = code.analysis_quality;
    }
    target.sources_examined = target.sources_examined.max(code.sources_examined);
    target.occurrences = target.occurrences.saturating_add(code.occurrences);
    target.truncated |= code.truncated;
    for gap in &code.gaps {
        if target.gaps.len() >= 256 {
            target.truncated = true;
            break;
        }
        if !target.gaps.contains(gap) {
            target.gaps.push(gap.clone());
        }
    }
}

const fn analysis_quality_rank(quality: AnalysisQuality) -> u8 {
    match quality {
        AnalysisQuality::Precise => 3,
        AnalysisQuality::Syntactic => 2,
        AnalysisQuality::Lexical => 1,
        AnalysisQuality::Unavailable => 0,
    }
}

fn validate_triangulation_target(target: &TriangulationTarget) -> Result<(), TexoError> {
    match target {
        TriangulationTarget::Claim { claim_id } if claim_id.is_empty() => Err(TexoError::OpInput {
            op: "texo.knowledge.triangulate".to_string(),
            detail: "claim_id must not be empty".to_string(),
        }),
        TriangulationTarget::Path {
            path,
            line_start,
            line_end,
        } => {
            let safe = !path.is_empty()
                && !Path::new(path).is_absolute()
                && Path::new(path)
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_)));
            let valid_range = match (*line_start, *line_end) {
                (None, None) => true,
                (Some(start), Some(end)) => start > 0 && start <= end,
                _ => false,
            };
            if safe && valid_range {
                Ok(())
            } else {
                Err(TexoError::OpInput {
                    op: "texo.knowledge.triangulate".to_string(),
                    detail: "path must be repository-relative and line bounds must be absent or an ordered one-based pair".to_string(),
                })
            }
        }
        TriangulationTarget::Symbol { symbol } if symbol.is_empty() || symbol.len() > 1024 => {
            Err(TexoError::OpInput {
                op: "texo.knowledge.triangulate".to_string(),
                detail: "symbol must contain between 1 and 1024 bytes".to_string(),
            })
        }
        TriangulationTarget::Claim { .. } | TriangulationTarget::Symbol { .. } => Ok(()),
    }
}

fn triangulation_claim_ids(
    view: &WorkspaceView,
    target: &TriangulationTarget,
) -> Result<BTreeSet<String>, TexoError> {
    match target {
        TriangulationTarget::Claim { claim_id } => {
            if view
                .claims
                .iter()
                .any(|claim| claim.card.claim_id == *claim_id)
            {
                Ok(BTreeSet::from([claim_id.clone()]))
            } else {
                Err(TexoError::MissingEntity {
                    entity: entity_for_claim(claim_id),
                })
            }
        }
        TriangulationTarget::Path {
            path,
            line_start,
            line_end,
        } => Ok(view
            .claims
            .iter()
            .filter(|claim| claim.card.source_path == *path)
            .filter(|claim| {
                line_start.is_none_or(|start| claim.card.line_end >= start)
                    && line_end.is_none_or(|end| claim.card.line_start <= end)
            })
            .map(|claim| claim.card.claim_id.clone())
            .collect()),
        TriangulationTarget::Symbol { .. } => Ok(BTreeSet::new()),
    }
}

fn evidence_matches_target(evidence: &ClaimEvidence, target: &TriangulationTarget) -> bool {
    match target {
        TriangulationTarget::Claim { .. } => true,
        TriangulationTarget::Path {
            path,
            line_start,
            line_end,
        } => {
            evidence.occurrence.path == *path
                && line_start.is_none_or(|start| evidence.occurrence.line_range.end >= start)
                && line_end.is_none_or(|end| evidence.occurrence.line_range.start <= end)
        }
        TriangulationTarget::Symbol { .. } => false,
    }
}

fn answer_state_for_rows(
    assertions: &[AgentClaimRow],
    evidence: &[ClaimEvidence],
    structural_evidence: &[CodeOccurrence],
) -> AnswerState {
    use crate::claims::status::ClaimStatus;
    if evidence
        .iter()
        .any(|item| item.stance == EvidenceStance::Contradicts)
    {
        AnswerState::Contradicted
    } else if assertions
        .iter()
        .any(|claim| claim.status == ClaimStatus::Conflicting)
    {
        AnswerState::Incomparable
    } else if assertions
        .iter()
        .any(|claim| claim.status == ClaimStatus::Superseded)
    {
        AnswerState::Stale
    } else if (!assertions.is_empty()
        && evidence
            .iter()
            .any(|item| item.stance == EvidenceStance::Supports))
        || !structural_evidence.is_empty()
    {
        AnswerState::Supported
    } else {
        AnswerState::Unverified
    }
}

fn answer_state_for_claim(
    status: Option<crate::claims::status::ClaimStatus>,
    evidence: &[ClaimEvidence],
) -> AnswerState {
    use crate::claims::status::ClaimStatus;
    match status {
        Some(ClaimStatus::Superseded) => AnswerState::Stale,
        Some(ClaimStatus::Conflicting) => AnswerState::Incomparable,
        Some(ClaimStatus::Current)
            if evidence
                .iter()
                .any(|item| item.stance == EvidenceStance::Contradicts) =>
        {
            AnswerState::Contradicted
        }
        Some(ClaimStatus::Current)
            if evidence
                .iter()
                .any(|item| item.stance == EvidenceStance::Supports) =>
        {
            AnswerState::Supported
        }
        Some(ClaimStatus::Current) | None => AnswerState::Unverified,
    }
}

struct EvidencePlan {
    rows: Vec<(EvidenceOccurrence, ClaimEvidenceLinkedV1)>,
    gaps: Vec<CoverageGap>,
}

fn append_knowledge_plan(
    cx: &mut syncbat::Ctx<'_>,
    workspace_id: &WorkspaceId,
    snapshot: &SourceSnapshotRecordedV1,
    rows: &[(EvidenceOccurrence, ClaimEvidenceLinkedV1)],
    relations: &[SourceSnapshotRelationV1],
    observed_at_ms: u64,
) -> Result<Vec<ReceiptNote>, TexoError> {
    append_json(
        "texo.knowledge.index",
        cx,
        <SourceSnapshotRecordedV1 as EventPayload>::KIND,
        snapshot,
    )?;
    for (occurrence, link) in rows {
        append_json(
            "texo.knowledge.index",
            cx,
            <EvidenceOccurrenceRecordedV1 as EventPayload>::KIND,
            &EvidenceOccurrenceRecordedV1 {
                workspace_id: workspace_id.clone(),
                occurrence: occurrence.clone(),
                observed_at_ms,
            },
        )?;
        append_json(
            "texo.knowledge.index",
            cx,
            <ClaimEvidenceLinkedV1 as EventPayload>::KIND,
            link,
        )?;
    }
    for relation in relations {
        append_json(
            "texo.knowledge.index",
            cx,
            <SourceSnapshotRelationV1 as EventPayload>::KIND,
            relation,
        )?;
    }
    take_receipts()
}

fn plan_claim_evidence(
    view: &WorkspaceView,
    sources: &[CapturedSource],
    snapshot_id: &crate::knowledge::SourceSnapshotId,
    observed_at_ms: u64,
) -> Result<EvidencePlan, TexoError> {
    let by_path = sources
        .iter()
        .map(|source| (source.path.as_str(), source))
        .collect::<BTreeMap<_, _>>();
    let workspace_id = WorkspaceId::new(view.workspace_id.clone())?;
    let mut rows = Vec::new();
    let mut gaps = Vec::new();
    for claim in &view.claims {
        let Some(source) = by_path.get(claim.card.source_path.as_str()) else {
            continue;
        };
        let source_digest_hex = crate::events::ids::blake3_bytes_hex(&source.bytes);
        let captured_source_id = crate::events::ids::source_id_from_hash(&source_digest_hex)?;
        if captured_source_id.as_str() != claim.card.source_id {
            gaps.push(CoverageGap {
                path: Some(source.path.clone()),
                kind: CoverageGapKind::AnalysisIncomplete,
            });
            continue;
        }
        let Some((start, end)) =
            line_byte_range(&source.bytes, claim.card.line_start, claim.card.line_end)
        else {
            gaps.push(CoverageGap {
                path: Some(source.path.clone()),
                kind: CoverageGapKind::AnalysisIncomplete,
            });
            continue;
        };
        let excerpt_bytes = &source.bytes[start..end];
        let Ok(excerpt) = std::str::from_utf8(excerpt_bytes) else {
            gaps.push(CoverageGap {
                path: Some(source.path.clone()),
                kind: CoverageGapKind::UnsupportedEncoding,
            });
            continue;
        };
        if excerpt.len() > MAX_EVIDENCE_EXCERPT_BYTES {
            gaps.push(CoverageGap {
                path: Some(source.path.clone()),
                kind: CoverageGapKind::SourceTooLarge,
            });
            continue;
        }
        let material = format!(
            "texo.evidence.occurrence.v1\u{1f}{snapshot_id}\u{1f}{}\u{1f}{start}\u{1f}{end}\u{1f}{}",
            source.path, claim.card.claim_id
        );
        let occurrence_id = EvidenceOccurrenceId::derive(&material);
        let occurrence = EvidenceOccurrence {
            occurrence_id: occurrence_id.clone(),
            snapshot_id: snapshot_id.clone(),
            source_kind: match source.layer {
                CapturedLayer::Committed => EvidenceSourceKind::GitBlob,
                CapturedLayer::Worktree => EvidenceSourceKind::WorktreeOverlay,
            },
            path: source.path.clone(),
            byte_range: ByteRange::new(
                u64::try_from(start).unwrap_or(u64::MAX),
                u64::try_from(end).unwrap_or(u64::MAX),
            )
            .map_err(|error| TexoError::Source {
                path: source.path.clone(),
                detail: error.to_string(),
            })?,
            line_range: LineRange::new(claim.card.line_start, claim.card.line_end).map_err(
                |error| TexoError::Source {
                    path: source.path.clone(),
                    detail: error.to_string(),
                },
            )?,
            git_blob: source.blob_id.clone(),
            source_digest_hex,
            excerpt: excerpt.to_string(),
            analyzer_fingerprint: format!(
                "{}:{}:{}",
                claim.card.extractor_kind, claim.card.extractor_model, claim.card.prompt_version
            ),
            analysis_quality: AnalysisQuality::Syntactic,
        };
        occurrence.validate().map_err(|error| TexoError::Source {
            path: source.path.clone(),
            detail: error.to_string(),
        })?;
        let link = ClaimEvidenceLinkedV1 {
            workspace_id: workspace_id.clone(),
            claim_id: ClaimId::try_from(claim.card.claim_id.as_str())?,
            occurrence_id,
            stance: EvidenceStance::Supports,
            method: EvidenceLinkMethod::Deterministic,
            observed_at_ms,
        };
        rows.push((occurrence, link));
    }
    Ok(EvidencePlan { rows, gaps })
}

fn line_byte_range(bytes: &[u8], start_line: u32, end_line: u32) -> Option<(usize, usize)> {
    if start_line == 0 || end_line < start_line {
        return None;
    }
    let mut line = 1_u32;
    let mut line_start = 0_usize;
    let mut range_start = None;
    for offset in 0..=bytes.len() {
        let boundary = offset == bytes.len() || bytes.get(offset) == Some(&b'\n');
        if !boundary {
            continue;
        }
        if line == start_line {
            range_start = Some(line_start);
        }
        if line == end_line {
            return range_start.map(|start| (start, offset));
        }
        line = line.saturating_add(1);
        line_start = offset.saturating_add(1);
    }
    None
}

struct SettlementAuthority {
    verdicts: BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    cache_keys: BTreeMap<(String, String), String>,
    warnings: Vec<crate::relate::settlement::AuthorityWarning>,
    unresolved_pairs: usize,
}

fn authoritative_settlements(frontier: Option<u64>) -> Result<SettlementAuthority, TexoError> {
    env::with(|op_env| {
        let scope = scope_for_workspace(&op_env.workspace_id);
        let region = Region::scope(&scope);
        let mut after = None;
        let mut entities = BTreeSet::new();
        loop {
            let page = op_env.store.query_entries_after(&region, after, 256);
            if page.is_empty() {
                break;
            }
            for entry in &page {
                if frontier.is_some_and(|frontier| entry.global_sequence() > frontier) {
                    break;
                }
                let entity = entry.coord().entity();
                if entity.starts_with("relation:") {
                    entities.insert(entity.to_string());
                }
            }
            if page
                .last()
                .is_some_and(|entry| frontier.is_some_and(|value| entry.global_sequence() > value))
            {
                break;
            }
            after = page.last().map(batpak::store::IndexEntry::global_sequence);
        }

        let mut settled = BTreeMap::new();
        let mut cache_keys = BTreeMap::new();
        let mut warnings = Vec::new();
        let mut unresolved_pairs = 0;
        for entity in entities {
            let card = if let Some(frontier) = frontier {
                let mut card = crate::claims::settlement::SettlementCard::default();
                for entry in op_env.store.by_entity(&entity) {
                    if entry.global_sequence() > frontier {
                        break;
                    }
                    let raw = op_env.store.read_raw(entry.event_id())?;
                    card.apply_event(&raw.event);
                }
                card
            } else {
                let Some(card) = op_env
                    .store
                    .project::<crate::claims::settlement::SettlementCard>(
                        &entity,
                        &Freshness::Consistent,
                    )?
                else {
                    continue;
                };
                card
            };
            let Some(authoritative) = card.authoritative.as_ref() else {
                if !card.deferrals.is_empty() {
                    unresolved_pairs += 1;
                }
                continue;
            };
            let older = ClaimId::try_from(card.older_claim.as_str())?;
            let newer = ClaimId::try_from(card.newer_claim.as_str())?;
            for later in &card.later_judgments {
                if later.relation != authoritative.relation {
                    warnings.push(crate::relate::settlement::AuthorityWarning {
                        old_claim: older.clone(),
                        new_claim: newer.clone(),
                        prior_verdict: authoritative.relation,
                        prior_fingerprint: authoritative.judge_fingerprint.clone(),
                        new_verdict: later.relation,
                        new_fingerprint: later.judge_fingerprint.clone(),
                        message: "authoritative verdict unchanged".to_string(),
                    });
                }
            }
            settled.insert(
                (older.clone(), newer.clone()),
                crate::semantics::RelationVerdict {
                    relation: authoritative.relation.into(),
                    score: ppm_to_score(authoritative.score_ppm),
                },
            );
            cache_keys.insert(
                (older.to_string(), newer.to_string()),
                authoritative.cache_key_hex.clone(),
            );
        }
        Ok::<_, TexoError>(SettlementAuthority {
            verdicts: settled,
            cache_keys,
            warnings,
            unresolved_pairs,
        })
    })?
}

fn require_complete_settlement() -> Result<(), TexoError> {
    let unresolved = authoritative_settlements(None)?.unresolved_pairs;
    if unresolved == 0 {
        return Ok(());
    }
    Err(TexoError::Semantics {
        backend: "settlement".to_string(),
        detail: format!(
            "strict settlement refused authority-bearing output: {unresolved} unresolved relation pair(s); run `texo relate` to resume"
        ),
    })
}

#[expect(
    clippy::cast_precision_loss,
    reason = "ppm values are bounded to one million and exactly adequate for model confidence"
)]
fn ppm_to_score(score_ppm: u32) -> f32 {
    score_ppm as f32 / 1_000_000.0
}

pub(crate) fn resolve_path(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

pub(crate) fn plan_sources(
    op: &str,
    root: &Path,
    input_path: &Path,
    workspace_id: &str,
    observed_at_ms: u64,
    extractor_cmd: Option<&str>,
    view: &WorkspaceView,
) -> Result<SourcePlan, TexoError> {
    let existing_hashes = view
        .sources
        .iter()
        .map(|source| source.body_hash_hex.clone())
        .collect::<BTreeSet<_>>();
    let mut batch_hashes = BTreeSet::new();
    let mut planned = Vec::new();
    let discover_started = Instant::now();
    let discovery = collect_markdown_files(input_path).map_err(|error| TexoError::Source {
        path: input_path.to_string_lossy().to_string(),
        detail: error.to_string(),
    })?;
    let discover_ms = elapsed_ms(discover_started);
    let empty = discovery.files.is_empty();
    let mut skipped = discovery
        .failures
        .into_iter()
        .map(|failure| SourceSkipRow {
            path: failure.path.to_string_lossy().to_string(),
            code: SourceFailureCode::Io,
            detail: bounded_source_detail(&failure.error.to_string()),
        })
        .collect::<Vec<_>>();
    let mut succeeded = 0;
    let extract_started = Instant::now();
    for path in discovery.files {
        let doc = match MarkdownDocument::from_path(&path, root) {
            Ok(doc) => doc,
            Err(error) => {
                skipped.push(SourceSkipRow {
                    path: path.to_string_lossy().to_string(),
                    code: match error {
                        crate::extract::markdown::SourceError::Utf8(_) => SourceFailureCode::Utf8,
                        crate::extract::markdown::SourceError::Io(_)
                        | crate::extract::markdown::SourceError::Walk(_)
                        | crate::extract::markdown::SourceError::Id(_) => SourceFailureCode::Io,
                    },
                    detail: bounded_source_detail(&error.to_string()),
                });
                continue;
            }
        };
        succeeded += 1;
        if existing_hashes.contains(&doc.body_hash_hex)
            || !batch_hashes.insert(doc.body_hash_hex.clone())
        {
            continue;
        }
        let claims = if let Some(cmd) = extractor_cmd {
            extract_cmd_claims(op, root, cmd, &path, &doc, workspace_id, observed_at_ms)?
        } else {
            extract_heuristic_claims(&doc, workspace_id, observed_at_ms)?
        };
        planned.push(PlannedSource {
            observed: SourceObservedV2 {
                source_id: doc.source_id,
                workspace_id: workspace_id.to_string(),
                source_kind: "markdown".to_string(),
                path: doc.path,
                body_hash_hex: doc.body_hash_hex,
                observed_at_ms,
            },
            claims,
        });
    }
    skipped.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(SourcePlan {
        sources: planned,
        skipped,
        empty,
        succeeded,
        discover_ms,
        extract_ms: elapsed_ms(extract_started),
    })
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn bounded_source_detail(detail: &str) -> String {
    detail
        .chars()
        .take(MAX_SOURCE_FAILURE_DETAIL_CHARS)
        .collect()
}

fn settle_source_failures(
    root: &Path,
    observed_at_ms: u64,
    rows: Vec<SourceSkipRow>,
) -> Result<(Vec<SourceSkipRow>, Option<String>), TexoError> {
    if rows.len() <= MAX_INLINE_SOURCE_FAILURES {
        return Ok((rows, None));
    }
    let bytes = serde_json::to_vec(&rows)?;
    let digest = blake3::hash(&bytes).to_hex().to_string();
    let short_digest: String = digest.chars().take(16).collect();
    let relative = PathBuf::from(".texo")
        .join("operations")
        .join("ingest-skips")
        .join(format!("{observed_at_ms}-{short_digest}.json"));
    let path = root.join(&relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, path)?;
    Ok((
        rows.into_iter().take(MAX_INLINE_SOURCE_FAILURES).collect(),
        Some(relative.to_string_lossy().to_string()),
    ))
}

/// Run the semantic relate pass and journal resulting v2 transition payloads.
///
/// # Errors
///
/// Returns [`TexoError::Semantics`] when the configured semantic backends fail,
/// [`TexoError::Store`] when projection/event reads fail, and
/// [`TexoError::OpRuntime`] when append effects fail or no receipt is produced.
#[expect(
    clippy::too_many_lines,
    reason = "WO-5 keeps relate orchestration in one op chokepoint"
)]
pub(crate) fn run_relate_pass(
    op: &'static str,
    cx: &mut syncbat::Ctx<'_>,
    observed_at_ms: u64,
    strict: bool,
) -> Result<RelateRunOutput, TexoError> {
    let view = assemble_current_view()?;
    let claims = semantic_claims_from_view(&view)?;
    if claims.len() < 2 {
        return Ok(RelateRunOutput {
            outcome: RelateCompletion::Complete,
            claims_related: claims.len(),
            supersessions: Vec::new(),
            conflicts: Vec::new(),
            unresolved: Vec::new(),
            held: Vec::new(),
            warnings: Vec::new(),
            authority_warnings: Vec::new(),
            receipts: Vec::new(),
        });
    }
    let (root, cluster, prefilter, gateway) = env::with(|op_env| {
        let cluster = op_env.config.semantics.as_ref().map_or_else(
            || crate::config::SemanticsConfig::default().cosine_threshold,
            |semantics| semantics.cosine_threshold,
        );
        let prefilter = op_env
            .config
            .semantics
            .as_ref()
            .and_then(|semantics| semantics.relate_prefilter)
            .unwrap_or(RELATE_PREFILTER);
        (
            op_env.root.clone(),
            cluster,
            prefilter,
            op_env.config.gateway.clone(),
        )
    })?;
    let authority = authoritative_settlements(None)?;
    let temporal = semantic_temporal_policy(&view, &claims)?;
    let budget_secs = std::env::var("TEXO_RELATE_BUDGET_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| gateway.as_ref().map(|config| config.relate_budget_secs))
        .unwrap_or(900);
    let mut related = relate_with_backends(
        &root,
        gateway.as_ref(),
        &claims,
        RelateThresholds { cluster, prefilter },
        &authority.verdicts,
        &temporal,
        std::time::Duration::from_secs(budget_secs),
    )?;
    for (pair, cache_key) in &authority.cache_keys {
        related.cache_keys.insert(pair.clone(), cache_key.clone());
    }
    let workspace_id = WorkspaceId::try_from(view.workspace_id.as_str())?;
    for judgment in &related.outcome.related.judgments {
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
                score_ppm: score_to_ppm(judgment.verdict.score),
                judge_fingerprint: related.judge_fingerprint.clone(),
                cache_key_hex,
                observed_at_ms,
            },
        )?;
    }
    for unresolved in &related.outcome.unresolved {
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
    let partial = !related.outcome.unresolved.is_empty();
    let allow_derived = !strict || !partial;
    let mut held = related.outcome.held.clone();
    if strict && partial {
        held.extend(related.outcome.related.supersessions.iter().map(
            |(old_claim, new_claim, reason)| {
                crate::relate::settlement::HeldDecision::Supersession {
                    old_claim: old_claim.clone(),
                    new_claim: new_claim.clone(),
                    reason: reason.clone(),
                }
            },
        ));
        held.extend(related.outcome.related.conflicts.iter().map(|conflict| {
            crate::relate::settlement::HeldDecision::Conflict {
                conflict_id: conflict.conflict_id.clone(),
                claim_a: conflict.claim_a.clone(),
                claim_b: conflict.claim_b.clone(),
                reason: conflict.reason.clone(),
            }
        }));
    }
    let existing_conflicts = view
        .conflicts
        .iter()
        .map(|conflict| conflict.conflict_id.clone())
        .collect::<BTreeSet<_>>();
    let mut supersessions = Vec::new();
    let supersession_decisions: &[_] = if allow_derived {
        &related.outcome.related.supersessions
    } else {
        &[]
    };
    for (old, new, reason) in supersession_decisions {
        let old_id = old.to_string();
        let new_id = new.to_string();
        let old_entity = entity_for_claim(&old_id);
        let cache_key = related
            .cache_keys
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
                workspace_id: view.workspace_id.clone(),
                reason: reason.clone(),
                decided_by: "texo-relate".to_string(),
                observed_at_ms,
                transition: transition_record(
                    CLAIM_MACHINE,
                    &old_entity,
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
        supersessions.push(RelateSupersessionRow {
            old_claim_id: old_id,
            new_claim_id: new_id,
            reason: reason.clone(),
            cache_key,
        });
    }

    let mut conflicts = Vec::new();
    let conflict_decisions: &[_] = if allow_derived {
        &related.outcome.related.conflicts
    } else {
        &[]
    };
    for conflict in conflict_decisions {
        let conflict_id = conflict.conflict_id.to_string();
        if existing_conflicts.contains(&conflict_id) {
            continue;
        }
        let claim_a = conflict.claim_a.to_string();
        let claim_b = conflict.claim_b.to_string();
        let cache_key = related
            .cache_keys
            .get(&(claim_a.clone(), claim_b.clone()))
            .cloned()
            .unwrap_or_default();
        append_json(
            op,
            cx,
            <ConflictOpenedV2 as EventPayload>::KIND,
            &ConflictOpenedV2 {
                conflict_id: conflict_id.clone(),
                workspace_id: view.workspace_id.clone(),
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
        conflicts.push(RelateConflictRow {
            conflict_id,
            claim_a,
            claim_b,
            reason: conflict.reason.clone(),
            cache_key,
        });
    }

    Ok(RelateRunOutput {
        outcome: if partial {
            RelateCompletion::Partial
        } else {
            RelateCompletion::Complete
        },
        claims_related: claims.len(),
        supersessions,
        conflicts,
        unresolved: related.outcome.unresolved,
        held,
        warnings: partial
            .then(|| {
                "semantic settlement is incomplete; unresolved pairs remain authoritative gaps"
                    .to_string()
            })
            .into_iter()
            .collect(),
        authority_warnings: authority.warnings,
        receipts: take_receipts()?,
    })
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "score is clamped to the closed 0..=1 interval before ppm conversion"
)]
fn score_to_ppm(score: f32) -> u32 {
    (score.clamp(0.0, 1.0) * 1_000_000.0).round() as u32
}

struct SemanticRelateOutput {
    outcome: crate::semantics::pipeline::RelateOutcome,
    cache_keys: BTreeMap<(String, String), String>,
    judge_fingerprint: String,
}

#[cfg(feature = "openrouter")]
fn relate_with_backends(
    root: &Path,
    gateway: Option<&crate::gateway::GatewayConfig>,
    claims: &[(ClaimId, SemanticClaimView)],
    thresholds: RelateThresholds,
    settled: &BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    temporal: &RelateTemporalPolicy,
    budget: std::time::Duration,
) -> Result<SemanticRelateOutput, TexoError> {
    use crate::extract::cache::CachingRelater;
    use crate::semantics::openrouter::{OpenRouterEmbedder, OpenRouterRelater};
    use crate::semantics::pipeline::relate_claims_settled_parallel_temporal;
    use crate::semantics::ClaimRelater as _;

    let embedder = OpenRouterEmbedder::new(None, gateway).map_err(semantic_error)?;
    let cache_dir = std::env::var_os(ENV_RELATE_CACHE)
        .map_or_else(|| root.join(DEFAULT_RELATE_CACHE), PathBuf::from);
    let caching_relater = CachingRelater::new(
        OpenRouterRelater::new(None, gateway).map_err(semantic_error)?,
        cache_dir,
    );
    let judge_fingerprint = caching_relater.fingerprint();
    // Judge calls are independent network waits; fan out across workers and
    // reassemble in pair order so settlement stays byte-identical. 4 default
    // workers keeps provider pressure polite; clamp guards misconfiguration.
    let concurrency = std::env::var("TEXO_RELATE_CONCURRENCY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(4)
        .clamp(1, 16);
    let relation_output = relate_claims_settled_parallel_temporal(
        claims,
        &embedder,
        &caching_relater,
        thresholds,
        settled,
        ParallelRelateOptions {
            temporal,
            budget,
            concurrency,
        },
    )
    .map_err(semantic_error)?;
    let cache_keys = relate_cache_keys(&caching_relater, claims, &relation_output);
    Ok(SemanticRelateOutput {
        outcome: relation_output,
        cache_keys,
        judge_fingerprint,
    })
}

#[cfg(not(feature = "openrouter"))]
fn relate_with_backends(
    _root: &Path,
    _gateway: Option<&crate::gateway::GatewayConfig>,
    _claims: &[(ClaimId, SemanticClaimView)],
    _thresholds: RelateThresholds,
    _settled: &BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    _temporal: &RelateTemporalPolicy,
    _budget: std::time::Duration,
) -> Result<SemanticRelateOutput, TexoError> {
    Err(TexoError::Semantics {
        backend: "openrouter".to_string(),
        detail: "openrouter feature is disabled".to_string(),
    })
}

#[cfg(feature = "openrouter")]
fn semantic_error(error: impl std::error::Error + Send + Sync + 'static) -> TexoError {
    TexoError::Semantics {
        backend: "openrouter".to_string(),
        detail: crate::error::error_chain(&error),
    }
}

fn semantic_claims_from_view(
    view: &WorkspaceView,
) -> Result<Vec<(ClaimId, SemanticClaimView)>, TexoError> {
    let receipts = claim_record_receipts()?;
    let mut claims = Vec::new();
    for claim in &view.claims {
        if claim.status != crate::claims::status::ClaimStatus::Current {
            continue;
        }
        let claim_id = ClaimId::try_from(claim.card.claim_id.as_str())?;
        let source_id = SourceId::try_from(claim.card.source_id.as_str())?;
        let receipt =
            receipts
                .get(&claim.card.claim_id)
                .ok_or_else(|| TexoError::MissingEntity {
                    entity: entity_for_claim(&claim.card.claim_id),
                })?;
        let supersedes = claim
            .supersedes
            .iter()
            .map(|id| ClaimId::try_from(id.as_str()))
            .collect::<Result<Vec<_>, _>>()?;
        let superseded_by = claim
            .card
            .superseded_by
            .as_deref()
            .map(ClaimId::try_from)
            .transpose()?;
        claims.push((
            claim_id.clone(),
            SemanticClaimView {
                claim_id,
                workspace_id: claim.card.workspace_id.clone(),
                source_id,
                source_path: claim.card.source_path.clone(),
                line_start: claim.card.line_start,
                line_end: claim.card.line_end,
                text: claim.card.text.clone(),
                normalized_text: claim.card.normalized_text.clone(),
                subject_hint: claim.card.subject_hint.clone().unwrap_or_default(),
                predicate_hint: claim.card.predicate_hint.clone().unwrap_or_default(),
                object_hint: claim.card.object_hint.clone().unwrap_or_default(),
                confidence_ppm: claim.card.confidence_ppm,
                extractor_kind: claim.card.extractor_kind.clone(),
                status: SemanticClaimStatus::Current,
                receipt: receipt_view(
                    0,
                    receipt.sequence,
                    "ClaimRecorded",
                    &scope_for_workspace(&view.workspace_id),
                    &entity_for_claim(&claim.card.claim_id),
                ),
                supersedes,
                superseded_by,
            },
        ));
    }
    claims.sort_by(|left, right| {
        left.1
            .receipt
            .sequence
            .get()
            .cmp(&right.1.receipt.sequence.get())
            .then_with(|| left.0.as_str().cmp(right.0.as_str()))
    });
    Ok(claims)
}

fn claim_record_receipts() -> Result<BTreeMap<String, AgentReceiptRow>, TexoError> {
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

#[cfg(feature = "openrouter")]
fn relate_cache_keys<R: crate::semantics::ClaimRelater>(
    caching_relater: &crate::extract::cache::CachingRelater<R>,
    claims: &[(ClaimId, SemanticClaimView)],
    relation_output: &crate::semantics::pipeline::RelateOutcome,
) -> BTreeMap<(String, String), String> {
    let by_id = claims
        .iter()
        .map(|(id, view)| (id.to_string(), view))
        .collect::<BTreeMap<_, _>>();
    let mut keys = BTreeMap::new();
    for judgment in &relation_output.related.judgments {
        if let (Some(old_view), Some(new_view)) = (
            by_id.get(judgment.older_claim.as_str()),
            by_id.get(judgment.newer_claim.as_str()),
        ) {
            keys.insert(
                (
                    judgment.older_claim.to_string(),
                    judgment.newer_claim.to_string(),
                ),
                caching_relater.cache_key(&old_view.text, &new_view.text),
            );
        }
    }
    keys
}

fn extract_heuristic_claims(
    doc: &MarkdownDocument,
    workspace_id: &str,
    observed_at_ms: u64,
) -> Result<Vec<ClaimRecordedV2>, TexoError> {
    let source_id = SourceId::try_from(doc.source_id.as_str())?;
    let mut claims = Vec::new();
    for line in &doc.lines {
        let normalized = normalize_line(&line.text);
        let Some(hints) = hints_from_line_normalized(&line.text, &normalized) else {
            continue;
        };
        let claim_id = claim_id_from_parts(&source_id, line.number, &normalized).to_string();
        let char_start = saturating_u32(line.char_start);
        let char_end = saturating_u32(line.char_start.saturating_add(line.text.len()));
        claims.push(ClaimRecordedV2 {
            claim_id,
            workspace_id: workspace_id.to_string(),
            source_id: doc.source_id.clone(),
            source_path: doc.path.clone(),
            line_start: line.number,
            line_end: line.number,
            char_start,
            char_end,
            text: line.text.clone(),
            normalized_text: normalized,
            subject_hint: Some(hints.subject_hint),
            predicate_hint: Some(hints.predicate_hint),
            object_hint: Some(hints.object_hint),
            confidence_ppm: hints.confidence_ppm,
            extractor_kind: "heuristic-v1".to_string(),
            extractor_model: String::new(),
            prompt_version: String::new(),
            observed_at_ms,
        });
    }
    Ok(claims)
}

/// Execute the workspace-configured extractor command.
///
/// This is an explicit local-code-execution trust boundary: anyone who can
/// write workspace configuration can execute as the Texo process via `sh -c`.
/// A future bvisor adapter belongs at this function boundary; this campaign
/// does not claim confinement for configured extractors.
fn extract_cmd_claims(
    op: &str,
    root: &Path,
    cmd: &str,
    path: &Path,
    doc: &MarkdownDocument,
    workspace_id: &str,
    observed_at_ms: u64,
) -> Result<Vec<ClaimRecordedV2>, TexoError> {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{cmd} \"$1\""))
        .arg("texo-extract")
        .arg(path)
        .current_dir(root)
        .output()
        .map_err(|error| TexoError::Extract {
            detail: format!("{op}: failed to run extractor: {error}"),
        })?;
    if !output.status.success() {
        return Err(TexoError::Extract {
            detail: format!("{op}: extractor exited with {}", output.status),
        });
    }
    let stdout = String::from_utf8(output.stdout).map_err(|error| TexoError::Extract {
        detail: format!("{op}: extractor stdout was not utf-8: {error}"),
    })?;
    let source_id = SourceId::try_from(doc.source_id.as_str())?;
    let mut claims = Vec::new();
    for (idx, line) in stdout.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed: CmdClaimLine =
            serde_json::from_str(line).map_err(|error| TexoError::Extract {
                detail: format!("{op}: extractor line {} json error: {error}", idx + 1),
            })?;
        let claim_id =
            claim_id_from_parts(&source_id, parsed.line_start, &parsed.normalized_text).to_string();
        claims.push(ClaimRecordedV2 {
            claim_id,
            workspace_id: workspace_id.to_string(),
            source_id: doc.source_id.clone(),
            source_path: doc.path.clone(),
            line_start: parsed.line_start,
            line_end: parsed.line_start,
            char_start: parsed.char_start.unwrap_or(0),
            char_end: parsed.char_end.unwrap_or(0),
            text: parsed.text,
            normalized_text: parsed.normalized_text,
            subject_hint: parsed.subject_hint,
            predicate_hint: parsed.predicate_hint,
            object_hint: parsed.object_hint,
            confidence_ppm: parsed.confidence_ppm,
            extractor_kind: "extractor-cmd".to_string(),
            extractor_model: parsed.extractor_model.unwrap_or_default(),
            prompt_version: parsed.prompt_version.unwrap_or_default(),
            observed_at_ms,
        });
    }
    Ok(claims)
}

fn saturating_u32(value: usize) -> u32 {
    // Source byte offsets are journaled as v2 u32 fields; extremely large inputs
    // saturate rather than truncating silently.
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub(crate) fn infer_supersessions(
    view: &WorkspaceView,
    new_claims: &[ClaimRecordedV2],
    observed_at_ms: u64,
) -> Vec<ClaimSupersededV2> {
    let mut by_subject: BTreeMap<Option<String>, Vec<&ClaimView>> = BTreeMap::new();
    for claim in &view.claims {
        by_subject
            .entry(claim.card.subject_hint.clone())
            .or_default()
            .push(claim);
    }
    let mut out = Vec::new();
    let mut seen_old = BTreeSet::new();
    for new_claim in new_claims {
        if !replacement_signal(&new_claim.normalized_text) {
            continue;
        }
        let Some(candidates) = by_subject.get(&new_claim.subject_hint) else {
            continue;
        };
        for old in candidates {
            if old.card.claim_id == new_claim.claim_id
                || old.card.phase != 1
                || old.card.normalized_text == new_claim.normalized_text
                || !seen_old.insert(old.card.claim_id.clone())
            {
                continue;
            }
            let old_entity = entity_for_claim(&old.card.claim_id);
            out.push(ClaimSupersededV2 {
                old_claim_id: old.card.claim_id.clone(),
                new_claim_id: new_claim.claim_id.clone(),
                workspace_id: new_claim.workspace_id.clone(),
                reason: "superseded by newer ingest claim".to_string(),
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
    out.sort_by(|left, right| {
        left.old_claim_id
            .cmp(&right.old_claim_id)
            .then_with(|| left.new_claim_id.cmp(&right.new_claim_id))
    });
    out
}

fn replacement_signal(normalized_text: &str) -> bool {
    crate::lexicon::contains_replacement_signal(normalized_text)
}

fn claim_list_rows(
    view: &WorkspaceView,
    subject: Option<&str>,
) -> Result<Vec<AgentClaimRow>, TexoError> {
    let mut rows = Vec::new();
    // One index scan for all receipts instead of one by_entity per claim.
    let receipts = claim_record_receipts()?;
    for claim in &view.claims {
        if subject.is_some_and(|wanted| claim.card.subject_hint.as_deref() != Some(wanted)) {
            continue;
        }
        let receipt = match receipts.get(&claim.card.claim_id) {
            Some(row) => row.clone(),
            None => claim_receipt(&claim.card.claim_id)?,
        };
        rows.push(AgentClaimRow {
            claim_id: claim.card.claim_id.clone(),
            status: claim.status,
            subject_hint: claim.card.subject_hint.clone(),
            text: claim.card.text.clone(),
            source: AgentSourceRow {
                source_id: claim.card.source_id.clone(),
                path: claim.card.source_path.clone(),
                line_start: claim.card.line_start,
            },
            receipt,
            supersedes: claim.supersedes.clone(),
            superseded_by: claim.card.superseded_by.clone(),
        });
    }
    Ok(rows)
}

fn search_knowledge_from_view(
    view: &WorkspaceView,
    snapshot: SnapshotRead,
    input: &KnowledgeSearchInput,
) -> Result<KnowledgeSearchOutput, TexoError> {
    let query = input.query.as_deref().unwrap_or("");
    if query.len() > 256 {
        return Err(TexoError::OpInput {
            op: "texo.knowledge.search".to_string(),
            detail: "query exceeds 256 bytes".to_string(),
        });
    }
    let limit = input.limit.unwrap_or(25);
    if !(1..=100).contains(&limit) {
        return Err(TexoError::OpInput {
            op: "texo.knowledge.search".to_string(),
            detail: "limit must be between 1 and 100".to_string(),
        });
    }
    let cursor_identity = knowledge_search_identity(&snapshot, query, input);
    let offset = parse_knowledge_search_cursor(input.cursor.as_deref(), &cursor_identity)?;
    let query_lower = query.to_ascii_lowercase();
    let mut ranked = claim_list_rows(view, input.subject.as_deref())?
        .into_iter()
        .filter(|claim| input.status.is_none_or(|status| claim.status == status))
        .filter_map(|claim| {
            claim_search_rank(&claim, &query_lower).map(|rank| RankedKnowledgeResult {
                rank,
                key: format!("claim:{}", claim.claim_id),
                result: KnowledgeSearchResult::Claim { claim },
            })
        })
        .collect::<Vec<_>>();
    let mut coverage = coverage_for_view(view, &snapshot);
    let mut code_index_available = false;
    if input.subject.is_none() && input.status.is_none() {
        if let Some(source_snapshot_id) = snapshot.descriptor.source_snapshot_id.as_ref() {
            let loaded = load_code_artifact_at(view.frontier, source_snapshot_id)?;
            if let Some(code_coverage) = &loaded.coverage {
                merge_coverage(&mut coverage, code_coverage);
            }
            if let Some(artifact) = loaded.artifact {
                code_index_available = true;
                ranked.extend(artifact.occurrences.into_iter().filter_map(|occurrence| {
                    code_search_rank(&occurrence, &query_lower).map(|rank| RankedKnowledgeResult {
                        rank,
                        key: format!(
                            "code:{}:{}:{}",
                            occurrence.symbol, occurrence.path, occurrence.byte_range.start
                        ),
                        result: KnowledgeSearchResult::Code { occurrence },
                    })
                }));
            }
        }
    }
    if !code_index_available
        && !coverage
            .gaps
            .iter()
            .any(|gap| gap.kind == CoverageGapKind::CodeIndexUnavailable)
    {
        coverage.gaps.push(CoverageGap {
            path: None,
            kind: CoverageGapKind::CodeIndexUnavailable,
        });
    }
    ranked.sort_by(|left, right| {
        left.rank
            .cmp(&right.rank)
            .then_with(|| left.key.cmp(&right.key))
    });
    let total = ranked.len();
    let results = ranked
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|ranked| ranked.result)
        .collect::<Vec<_>>();
    let returned = results.len();
    let next_offset = offset.saturating_add(returned);
    let has_more = next_offset < total;
    Ok(KnowledgeSearchOutput {
        workspace_id: view.workspace_id.clone(),
        frontier: view.frontier,
        total,
        returned,
        has_more,
        next_cursor: has_more.then(|| format!("texo-knowledge-v1:{cursor_identity}:{next_offset}")),
        results,
        code_index_available,
        coverage,
        snapshot,
    })
}

fn knowledge_search_identity(
    snapshot: &SnapshotRead,
    query: &str,
    input: &KnowledgeSearchInput,
) -> String {
    let material = format!(
        "texo.knowledge.search.v1\u{1f}{}\u{1f}{query}\u{1f}{}\u{1f}{}",
        snapshot.token.as_str(),
        input.subject.as_deref().unwrap_or(""),
        input
            .status
            .map_or("", crate::claims::status::ClaimStatus::as_str)
    );
    blake3::hash(material.as_bytes()).to_hex()[..16].to_string()
}

fn parse_knowledge_search_cursor(
    cursor: Option<&str>,
    expected_identity: &str,
) -> Result<usize, TexoError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let Some(rest) = cursor.strip_prefix("texo-knowledge-v1:") else {
        return Err(invalid_knowledge_cursor());
    };
    let Some((identity, offset)) = rest.split_once(':') else {
        return Err(invalid_knowledge_cursor());
    };
    if identity != expected_identity {
        return Err(invalid_knowledge_cursor());
    }
    offset.parse().map_err(|_| invalid_knowledge_cursor())
}

fn invalid_knowledge_cursor() -> TexoError {
    TexoError::OpInput {
        op: "texo.knowledge.search".to_string(),
        detail: "cursor is invalid for this query and snapshot".to_string(),
    }
}

fn claim_search_rank(claim: &AgentClaimRow, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    let text = claim.text.to_ascii_lowercase();
    let subject = claim
        .subject_hint
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let path = claim.source.path.to_ascii_lowercase();
    if text == query || subject == query {
        Some(0)
    } else if text.starts_with(query) || subject.starts_with(query) {
        Some(1)
    } else if text.contains(query) || subject.contains(query) {
        Some(2)
    } else if path.contains(query) {
        Some(3)
    } else {
        None
    }
}

fn code_search_rank(occurrence: &CodeOccurrence, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(1);
    }
    let name = occurrence.display_name.to_ascii_lowercase();
    let symbol = occurrence.symbol.to_ascii_lowercase();
    let path = occurrence.path.to_ascii_lowercase();
    if name == query || symbol == query {
        Some(0)
    } else if name.starts_with(query) {
        Some(1)
    } else if name.contains(query) || symbol.contains(query) {
        Some(2)
    } else if path.contains(query) {
        Some(3)
    } else {
        None
    }
}

fn parse_claim_search_cursor(cursor: Option<&str>) -> Result<usize, TexoError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let Some(offset) = cursor.strip_prefix("texo-claims-v1:") else {
        return Err(TexoError::OpInput {
            op: "texo.claims.search".to_string(),
            detail: "cursor has an unsupported schema".to_string(),
        });
    };
    offset.parse::<usize>().map_err(|_| TexoError::OpInput {
        op: "texo.claims.search".to_string(),
        detail: "cursor offset is invalid".to_string(),
    })
}

fn claim_matches_query(row: &AgentClaimRow, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let mut searchable = row.text.to_ascii_lowercase();
    searchable.push(' ');
    searchable.push_str(&row.source.path.to_ascii_lowercase());
    if let Some(subject) = &row.subject_hint {
        searchable.push(' ');
        searchable.push_str(&subject.to_ascii_lowercase());
    }
    terms.iter().all(|term| searchable.contains(term))
}

fn build_agent_context_from_view(
    view: &WorkspaceView,
    subject: Option<&str>,
    include_stale: bool,
    snapshot: SnapshotRead,
) -> Result<AgentContextOutput, TexoError> {
    let claims = claim_list_rows(view, subject)?
        .into_iter()
        .filter(|claim| claim.status != crate::claims::status::ClaimStatus::Superseded)
        .collect::<Vec<_>>();
    let stale_claims = if include_stale {
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
    } else {
        Vec::new()
    };

    let mut conflicts = Vec::new();
    for conflict in &view.conflicts {
        if conflict.phase != 1 {
            continue;
        }
        conflicts.push(AgentConflictRow {
            conflict_id: conflict.conflict_id.clone(),
            claim_a: conflict.claim_a.clone(),
            claim_a_text: claim_text(view, &conflict.claim_a),
            claim_b: conflict.claim_b.clone(),
            claim_b_text: claim_text(view, &conflict.claim_b),
            reason: conflict.reason.clone(),
        });
    }
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

fn check_staleness_from_view(
    view: &WorkspaceView,
    workspace_id: &str,
    root: &Path,
    input: &Path,
    snapshot: SnapshotRead,
) -> Result<StalenessReport, TexoError> {
    let checked_path = input
        .strip_prefix(root)
        .unwrap_or(input)
        .to_string_lossy()
        .to_string();
    let discovery = collect_markdown_files(input).map_err(|error| TexoError::Source {
        path: input.to_string_lossy().to_string(),
        detail: error.to_string(),
    })?;
    if let Some(failure) = discovery.failures.first() {
        return Err(TexoError::Source {
            path: failure.path.to_string_lossy().to_string(),
            detail: failure.error.to_string(),
        });
    }
    let by_id = view
        .claims
        .iter()
        .map(|claim| (claim.card.claim_id.clone(), claim))
        .collect::<BTreeMap<_, _>>();
    let mut diagnostics = Vec::new();
    for path in discovery.files {
        let doc = MarkdownDocument::from_path(&path, root).map_err(|error| TexoError::Source {
            path: path.to_string_lossy().to_string(),
            detail: error.to_string(),
        })?;
        let source_id = SourceId::try_from(doc.source_id.as_str())?;
        // Match superseded claims of THIS doc by normalized-text containment in
        // the doc's current lines. Reconstructing claim ids from whole lines
        // only matches heuristic whole-line claims; LLM extraction proposes
        // sub-sentence claims whose identity a line-level rebuild never hits.
        // Normalize each doc line once, not once per superseded claim.
        let normalized_lines = doc
            .lines
            .iter()
            .map(|line| (line, normalize_line(&line.text)))
            .collect::<Vec<_>>();
        for claim in &view.claims {
            if claim.card.phase != 2 || claim.card.source_id != source_id.as_str() {
                continue;
            }
            let needle = claim.card.normalized_text.as_str();
            if needle.is_empty() {
                continue;
            }
            let line = normalized_lines
                .iter()
                .find(|(line, normalized)| {
                    line.number == claim.card.line_start && normalized.contains(needle)
                })
                .or_else(|| {
                    normalized_lines
                        .iter()
                        .find(|(_, normalized)| normalized.contains(needle))
                })
                .map(|(line, _)| *line);
            let Some(line) = line else {
                continue; // the stale text no longer appears in the doc
            };
            let superseded_by = claim.card.superseded_by.clone();
            let source = superseded_by
                .as_ref()
                .and_then(|id| by_id.get(id))
                .map(|superseder| DiagnosticSource {
                    path: superseder.card.source_path.clone(),
                    line_start: superseder.card.line_start,
                });
            let receipt = superseded_by
                .as_ref()
                .and_then(|id| claim_receipt(id).ok())
                .or_else(|| claim_receipt(&claim.card.claim_id).ok());
            let message = format!(
                "Claim appears stale: superseded by {} at {}.",
                superseded_by.as_deref().unwrap_or("unknown"),
                receipt.as_ref().map_or_else(
                    || "unknown seq".to_string(),
                    |receipt| format!("local seq {}", receipt.sequence)
                )
            );
            diagnostics.push(StaleDiagnostic {
                file: doc.path.clone(),
                line_start: line.number,
                line_end: line.number,
                severity: DiagnosticSeverity::Warning,
                message,
                claim_id: claim.card.claim_id.clone(),
                superseded_by,
                source,
                receipt,
            });
        }
    }
    Ok(StalenessReport {
        workspace_id: workspace_id.to_string(),
        checked_path,
        replayed_through_sequence: view.frontier,
        diagnostics,
        snapshot,
    })
}

fn compile_artifacts(
    context: &AgentContextOutput,
    view: &WorkspaceView,
    stale: &StalenessReport,
    conflicts: &heuristic::ConflictReport,
) -> Result<Vec<CompileFile>, TexoError> {
    Ok(vec![
        CompileFile {
            name: "onboarding.generated.md".to_string(),
            contents: render_onboarding(context),
        },
        CompileFile {
            name: "claims.json".to_string(),
            contents: serde_json::to_string_pretty(view)?,
        },
        CompileFile {
            name: "stale-context.json".to_string(),
            contents: serde_json::to_string_pretty(stale)?,
        },
        CompileFile {
            name: "conflicts.json".to_string(),
            contents: serde_json::to_string_pretty(conflicts)?,
        },
        CompileFile {
            name: "agent-context.json".to_string(),
            contents: serde_json::to_string_pretty(context)?,
        },
        CompileFile {
            name: "index.html".to_string(),
            contents: render_index_html(context, stale, conflicts)?,
        },
    ])
}

fn render_onboarding(context: &AgentContextOutput) -> String {
    let mut out = String::from("# Generated Onboarding\n\n");
    out.push_str(
        "_This document is a projection replayed from the texo claim-chain. \
         It is not source truth._\n\n",
    );
    writeln!(
        &mut out,
        "_Replayed through local store sequence {}._\n",
        context.replayed_through_sequence
    )
    .expect("writing to a String cannot fail");
    out.push_str("## Current claims\n\n");
    for claim in &context.claims {
        writeln!(
            &mut out,
            "- **{}** ({}): {}  \n  _source: {}:{}_",
            claim.claim_id,
            claim.subject_hint.clone().unwrap_or_default(),
            claim.text,
            claim.source.path,
            claim.source.line_start
        )
        .expect("writing to a String cannot fail");
    }
    if !context.stale_claims.is_empty() {
        out.push_str("\n## Stale claims (do not trust)\n\n");
        for stale in &context.stale_claims {
            writeln!(
                &mut out,
                "- {}: \"{}\" superseded by {}",
                stale.claim_id, stale.text, stale.superseded_by
            )
            .expect("writing to a String cannot fail");
        }
    }
    if !context.conflicts.is_empty() {
        out.push_str("\n## Conflicts (unresolved — both claimed, neither wins)\n\n");
        for conflict in &context.conflicts {
            writeln!(
                &mut out,
                "- \"{}\" ({}) vs \"{}\" ({})",
                conflict.claim_a_text, conflict.claim_a, conflict.claim_b_text, conflict.claim_b
            )
            .expect("writing to a String cannot fail");
        }
    }
    out
}

fn render_index_html(
    context: &AgentContextOutput,
    stale: &StalenessReport,
    conflicts: &heuristic::ConflictReport,
) -> Result<String, TexoError> {
    let mut claim_cards = String::new();
    for claim in &context.claims {
        let supersedes = if claim.supersedes.is_empty() {
            String::new()
        } else {
            format!(
                "<p><strong>supersedes:</strong> {}</p>",
                claim.supersedes.join(", ")
            )
        };
        write!(
            &mut claim_cards,
            r#"<article class="claim-card">
  <h2>Claim {id}</h2>
  <p><strong>status:</strong> current</p>
  <p><strong>subject:</strong> {subject}</p>
  <p><strong>local sequence:</strong> {seq}</p>
  <p><strong>frontier:</strong> replayed through seq {frontier}</p>
  <p><strong>source:</strong> {path}:{line}</p>
  <p><strong>receipt:</strong> {receipt}</p>
  {supersedes}
  <blockquote>{text}</blockquote>
</article>"#,
            id = claim.claim_id,
            subject = claim.subject_hint.clone().unwrap_or_default(),
            seq = claim.receipt.sequence,
            frontier = context.replayed_through_sequence,
            path = claim.source.path,
            line = claim.source.line_start,
            receipt = claim.receipt.event_id,
            supersedes = supersedes,
            text = html_escape(&claim.text),
        )
        .expect("writing to a String cannot fail");
    }
    let mut stale_cards = String::new();
    for diag in &stale.diagnostics {
        write!(
            &mut stale_cards,
            r#"<article class="claim-card stale">
  <h2>Stale line {}:{}</h2>
  <p>{}</p>
</article>"#,
            diag.file,
            diag.line_start,
            html_escape(&diag.message)
        )
        .expect("writing to a String cannot fail");
    }
    let conflicts_json = serde_json::to_string_pretty(conflicts)?;
    Ok(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>texo claim explorer</title>
  <style>
    body {{ font-family: system-ui, sans-serif; max-width: 960px; margin: 2rem auto; padding: 0 1rem; }}
    .claim-card {{ border: 1px solid #ccc; border-radius: 8px; padding: 1rem; margin-bottom: 1rem; }}
    .stale {{ border-color: #c90; background: #fff8e6; }}
    footer {{ margin-top: 3rem; color: #666; font-size: 0.9rem; }}
  </style>
</head>
<body>
  <header>
    <h1>A block explorer for stale team beliefs.</h1>
    <p>Every claim below was replayed from a BatPak journal.        The generated onboarding doc is a projection, not source truth.</p>
  </header>
  <section>
    <h2>Current claims</h2>
    {claim_cards}
  </section>
  <section>
    <h2>Stale diagnostics</h2>
    {stale_cards}
  </section>
  <section>
    <h2>Conflicts ({conflict_count})</h2>
    <pre>{conflicts_json}</pre>
  </section>
  <footer>
    texo uses one local BatPak journal. Sequences are per-store.     No global order, network consensus, or distributed replication is claimed.
  </footer>
</body>
</html>"#,
        conflict_count = conflicts.conflicts.len(),
        conflicts_json = html_escape(&conflicts_json)
    ))
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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

fn conflict_status(conflict: &ConflictCard) -> crate::claims::status::ConflictStatus {
    match conflict.phase {
        2 => crate::claims::status::ConflictStatus::Resolved,
        3 => crate::claims::status::ConflictStatus::Ignored,
        _ => crate::claims::status::ConflictStatus::Open,
    }
}

fn claim_phase_name(phase: u64) -> &'static str {
    match phase {
        0 => "unrecorded",
        1 => "current",
        2 => "superseded",
        _ => "invalid",
    }
}

fn conflict_phase_name(phase: u64) -> &'static str {
    match phase {
        0 => "unopened",
        1 => "open",
        2 => "resolved",
        3 => "ignored",
        _ => "invalid",
    }
}

fn workspace_event_count() -> Result<usize, TexoError> {
    env::with(|op_env| {
        let region = Region::scope(scope_for_workspace(&op_env.workspace_id));
        let mut after = None;
        let mut count = 0usize;
        loop {
            let page = op_env.store.query_entries_after(&region, after, 256);
            if page.is_empty() {
                break;
            }
            count = count.saturating_add(page.len());
            after = page.last().map(batpak::store::IndexEntry::global_sequence);
        }
        count
    })
}

fn file_bytes(path: &Path) -> Result<u64, TexoError> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error.into()),
    }
}

fn journal_file_bytes(path: &Path) -> Result<u64, TexoError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    if metadata.is_file() {
        return Ok(
            if path.extension().and_then(std::ffi::OsStr::to_str) == Some("fbat") {
                metadata.len()
            } else {
                0
            },
        );
    }
    let mut bytes = 0u64;
    for entry in std::fs::read_dir(path)? {
        bytes = bytes.saturating_add(journal_file_bytes(&entry?.path())?);
    }
    Ok(bytes)
}

fn conflict_subject(view: &WorkspaceView, conflict: &ConflictCard) -> String {
    view.claims
        .iter()
        .find(|claim| claim.card.claim_id == conflict.claim_a)
        .and_then(|claim| claim.card.subject_hint.clone())
        .unwrap_or_default()
}

fn claim_text(view: &WorkspaceView, claim_id: &str) -> String {
    view.claims
        .iter()
        .find(|claim| claim.card.claim_id == claim_id)
        .map(|claim| claim.card.text.clone())
        .unwrap_or_default()
}

fn newer_claim<'a>(
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

fn claim_receipt(claim_id: &str) -> Result<AgentReceiptRow, TexoError> {
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
