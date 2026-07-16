use super::common::{
    append_json, assemble_current_view, assemble_snapshot_view, evidence_projection_through,
    latest_source_snapshot, op_runtime, parse_input, plan_and_attach_snapshot_relations,
    recorded_capture_limits, run_op, source_snapshot_by_id, source_snapshots_through,
    take_one_receipt, take_receipts, WORKSPACE_VIEW_PROJECTION,
};
use super::ingest::{settle_indexed_explicit_supersessions, HeldExplicitSupersession};
use super::knowledge_read::{
    append_knowledge_plan, latest_code_index, load_code_artifact_at, plan_claim_evidence,
    triangulate_from_view,
};
use super::model::AgentClaimRow;
use crate::code_index::{
    build as build_code_index, load as load_code_index, persist as persist_code_index, read_scip,
    CodeIndexLimits,
};
use crate::error::{SnapshotFailureKind, TexoError};
use crate::events::ids::WorkspaceId;
use crate::events::payloads::{CodeIndexRecordedV1, SourceSnapshotRecordedV1};
use crate::git_source::{capture as capture_git, CaptureLimits, GitCapture};
use crate::knowledge::{
    AnalysisQuality, AnswerState, ClaimEvidence, CodeIndexId, CodeOccurrence, CoverageGap,
    CoverageGapKind, KnowledgeCoverage, RepositoryId, SnapshotRead, TriangulationTarget,
    UncertaintyReason,
};
use crate::ops::env;
use crate::ops::env::ReceiptNote;
use crate::ops::reconcile::append_proposals as append_reconciliation_proposals;
use crate::reconcile::{
    claims_from_view as reconcile_claims, evaluate_with_backends, plan_candidates,
    unresolved_row as reconcile_unresolved_row, KnowledgeReconcileInput, KnowledgeReconcileOutput,
    ReconcileBackendOutput, ReconcileCompletion,
};
use batpak::event::EventPayload;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use syncbat::HandlerResult;

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
    descriptor = KNOWLEDGE_INDEX,
    register = register_knowledge_index,
    register_item = knowledge_index_item,
    name = "texo.knowledge.index",
    effect = Persist,
    input_schema = "texo.knowledge.index.input.v1",
    output_schema = "texo.knowledge.index.output.v2",
    receipt_kind = "receipt.texo.knowledge.index.v1",
    appends_events = ["evt.e003", "evt.e00b", "evt.e00c", "evt.e00d", "evt.e00f"],
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
        let repository_id = repository_id_for_index(&root, &workspace_id, &previous_snapshots);
        let mut capture = capture_git(&root, repository_id, limits)?;
        let already_indexed = latest_source_snapshot(Some(view.frontier))?
            .is_some_and(|existing| existing.snapshot_id == capture.snapshot_id);

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
            capture_max_files: u64::try_from(limits.max_files).unwrap_or(u64::MAX),
            capture_max_file_bytes: limits.max_file_bytes,
            capture_max_total_bytes: limits.max_total_bytes,
            observed_at_ms: input.observed_at_ms,
        };
        let indexed_claim_ids = planned
            .rows
            .iter()
            .map(|(_, link)| link.claim_id.clone())
            .collect::<BTreeSet<_>>();
        let mut receipts = append_knowledge_plan(
            cx,
            &workspace,
            &snapshot,
            &planned.rows,
            &relations,
            input.observed_at_ms,
        )?;
        let supersessions = settle_indexed_explicit_supersessions(
            cx,
            &view,
            &indexed_claim_ids,
            input.observed_at_ms,
            &mut receipts,
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
            supersessions_applied: supersessions.applied.len(),
            supersessions_held: supersessions.held.len(),
            held_supersessions: supersessions.held,
            already_indexed,
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
    output_schema = "texo.code.index.build.output.v2",
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
        let (recorded, capture) = recorded_source_capture(&root, &input)?;
        if input.can_reuse_default() {
            if let Some(existing) = latest_code_index(None, &recorded.snapshot_id)? {
                if let Some(artifact) =
                    load_code_index(&root, &existing.index_id, &existing.artifact_digest_hex)?
                {
                    if artifact.snapshot_id != recorded.snapshot_id {
                        return Err(TexoError::Snapshot {
                            kind: SnapshotFailureKind::SourceUnavailable,
                            detail: "code-index artifact belongs to a different source snapshot"
                                .to_string(),
                        });
                    }
                    return Ok(CodeIndexBuildOutput {
                        workspace_id,
                        snapshot_id: existing.snapshot_id,
                        index_id: existing.index_id.clone(),
                        format: artifact.format,
                        analyzer_fingerprint: artifact.analyzer_fingerprint,
                        artifact_digest_hex: existing.artifact_digest_hex,
                        artifact_path: format!(
                            ".texo/cache/code-index/{}.bin",
                            existing.index_id.as_str()
                        ),
                        coverage: artifact.coverage,
                        already_indexed: true,
                        receipt: None,
                    });
                }
            }
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
            already_indexed: false,
            receipt: Some(take_one_receipt("texo.code.index.build")?),
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

fn repository_id_for_index(
    root: &Path,
    workspace_id: &str,
    previous: &[SourceSnapshotRecordedV1],
) -> RepositoryId {
    previous.last().cloned().map_or_else(
        || {
            let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
            RepositoryId::derive(&format!(
                "texo.repository.v1\u{1f}{workspace_id}\u{1f}{}",
                canonical.display()
            ))
        },
        |snapshot| snapshot.repository_id,
    )
}

fn recorded_source_capture(
    root: &Path,
    input: &CodeIndexBuildInput,
) -> Result<(SourceSnapshotRecordedV1, GitCapture), TexoError> {
    let recorded = match input.snapshot_id.as_ref() {
        Some(wanted) => source_snapshot_by_id(wanted)?.ok_or_else(|| TexoError::Snapshot {
            kind: SnapshotFailureKind::SourceUnavailable,
            detail: "requested source snapshot is not recorded; run `texo index` first".to_string(),
        })?,
        None => latest_source_snapshot(None)?.ok_or_else(|| TexoError::Snapshot {
            kind: SnapshotFailureKind::SourceUnavailable,
            detail: "run `texo index` to record a Git source snapshot first".to_string(),
        })?,
    };
    let limits = recorded_capture_limits(&recorded);
    let capture = capture_git(root, recorded.repository_id.clone(), limits)?;
    if capture.snapshot_id != recorded.snapshot_id {
        return Err(TexoError::Snapshot {
            kind: SnapshotFailureKind::SourceUnavailable,
            detail:
                "Git commit/index/worktree changed; run source indexing again before code indexing"
                    .to_string(),
        });
    }
    Ok((recorded, capture))
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
    supersessions_applied: usize,
    supersessions_held: usize,
    held_supersessions: Vec<HeldExplicitSupersession>,
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
    fn can_reuse_default(&self) -> bool {
        self.scip_path.is_none()
            && self.max_scip_bytes.is_none()
            && self.max_documents.is_none()
            && self.max_occurrences.is_none()
            && self.analysis_budget_secs.is_none()
    }

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
    already_indexed: bool,
    receipt: Option<ReceiptNote>,
}
#[derive(Debug, Deserialize)]
struct KnowledgeTriangulateInput {
    target: TriangulationTarget,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct KnowledgeTriangulateOutput {
    pub(super) target: TriangulationTarget,
    pub(super) answer_state: AnswerState,
    pub(super) assertions: Vec<AgentClaimRow>,
    pub(super) evidence: Vec<ClaimEvidence>,
    pub(super) structural_evidence: Vec<CodeOccurrence>,
    pub(super) uncertainty: Vec<UncertaintyReason>,
    pub(super) coverage: KnowledgeCoverage,
    pub(super) settlement_complete: bool,
    pub(super) snapshot: SnapshotRead,
}
