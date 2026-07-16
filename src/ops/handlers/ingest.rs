use super::common::{
    append_json, assemble_current_view, elapsed_ms, op_runtime, parse_input, run_op, take_receipts,
    workspace_temporal_policy, WORKSPACE_VIEW_PROJECTION,
};
use crate::claims::workspace::WorkspaceView;
use crate::error::TexoError;
use crate::events::ids::{claim_id_from_parts, ClaimId, SourceId};
use crate::events::payloads::{ClaimRecordedV2, ClaimSupersededV2, SourceObservedV2};
use crate::extract::hints::hints_from_line_normalized;
use crate::extract::markdown::{collect_markdown_files, MarkdownDocument};
use crate::extract::normalize::normalize_line;
use crate::ops::env;
use crate::ops::env::ReceiptNote;
use batpak::event::EventPayload;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;
use syncbat::HandlerResult;

mod supersession;

pub(crate) use supersession::infer_supersessions;
pub(super) use supersession::settle_indexed_explicit_supersessions;

const MAX_INLINE_SOURCE_FAILURES: usize = 256;
const MAX_SOURCE_FAILURE_DETAIL_CHARS: usize = 512;

#[syncbat::operation(
    descriptor = INGEST_RUN,
    register = register_ingest_run,
    register_item = ingest_run_item,
    name = "texo.ingest.run",
    effect = Persist,
    input_schema = "texo.ingest.run.input.v2",
    output_schema = "texo.ingest.run.output.v3",
    receipt_kind = "receipt.texo.ingest.run.v2",
    appends_events = ["evt.e001", "evt.e002", "evt.e003"],
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
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
        let project_ms = elapsed_ms(project_started);
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
        reject_failed_ingest_plan(&root, &path, &plan, &input)?;
        let execution = execute_ingest_plan(
            cx,
            &plan,
            &config,
            &mut view,
            input.observed_at_ms,
            input.dry_run,
            project_ms,
        )?;
        finish_ingest_output(&root, workspace_id, plan, &input, execution)
    })
}

fn reject_failed_ingest_plan(
    root: &Path,
    path: &Path,
    plan: &SourcePlan,
    input: &IngestRunInput,
) -> Result<(), TexoError> {
    if plan.skipped.is_empty() || (!input.strict && (plan.succeeded > 0 || plan.empty)) {
        return Ok(());
    }
    let sample = serde_json::to_string(&plan.skipped.iter().take(8).cloned().collect::<Vec<_>>())?;
    let (_, artifact) = settle_source_failures(root, input.observed_at_ms, plan.skipped.clone())?;
    Err(TexoError::Source {
        path: path.to_string_lossy().to_string(),
        detail: format!(
            "{} source(s) failed during planning; strict={} good_sources={}; sample={sample}; artifact={}",
            plan.skipped.len(),
            input.strict,
            plan.succeeded,
            artifact.as_deref().unwrap_or("inline")
        ),
    })
}

#[derive(Debug, Deserialize)]
pub(crate) struct IngestRunInput {
    pub(crate) path: PathBuf,
    pub(crate) dry_run: bool,
    #[serde(default)]
    pub(crate) strict: bool,
    pub(crate) observed_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum IngestCompletion {
    Complete,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum SourceFailureCode {
    #[serde(rename = "source.utf8")]
    Utf8,
    #[serde(rename = "source.io")]
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SourceSkipRow {
    pub(crate) path: String,
    pub(crate) code: SourceFailureCode,
    pub(crate) detail: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct IngestRunOutput {
    pub(crate) outcome: IngestCompletion,
    pub(crate) workspace_id: String,
    pub(crate) sources_observed: u32,
    pub(crate) claims_recorded: u32,
    pub(crate) claims_superseded: u32,
    pub(crate) supersessions_held: usize,
    pub(crate) held_supersessions: Vec<HeldExplicitSupersession>,
    pub(crate) dry_run: bool,
    pub(crate) empty: bool,
    pub(crate) skipped: Vec<SourceSkipRow>,
    pub(crate) skipped_total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) skipped_artifact: Option<String>,
    pub(crate) phase_ms: IngestPhaseMs,
    pub(crate) events_appended: u64,
    pub(crate) receipts: Vec<ReceiptNote>,
}

#[derive(Debug, Serialize)]
pub(crate) struct IngestPhaseMs {
    pub(crate) discover: u64,
    pub(crate) extract: u64,
    pub(crate) append: u64,
    pub(crate) project: u64,
}

pub(crate) struct PlannedSource {
    pub(crate) observed: SourceObservedV2,
    pub(crate) claims: Vec<ClaimRecordedV2>,
}

pub(crate) struct SourcePlan {
    pub(crate) sources: Vec<PlannedSource>,
    pub(crate) skipped: Vec<SourceSkipRow>,
    pub(crate) empty: bool,
    pub(crate) succeeded: usize,
    pub(crate) discover_ms: u64,
    pub(crate) extract_ms: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CmdClaimLine {
    pub(crate) line_start: u32,
    pub(crate) text: String,
    pub(crate) normalized_text: String,
    pub(crate) subject_hint: Option<String>,
    pub(crate) predicate_hint: Option<String>,
    pub(crate) object_hint: Option<String>,
    pub(crate) confidence_ppm: u32,
    pub(crate) char_start: Option<u32>,
    pub(crate) char_end: Option<u32>,
    pub(crate) extractor_model: Option<String>,
    pub(crate) prompt_version: Option<String>,
}
/// Closed reason an explicit replacement proposal could not become authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplicitSupersessionHoldReason {
    /// The proposed successor is on an older source revision.
    TemporalReversed,
    /// Both source revisions are valid but incomparable.
    TemporalConcurrent,
    /// Available source evidence cannot establish an order.
    TemporalUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
/// One explicit replacement proposal held by temporal policy.
pub struct HeldExplicitSupersession {
    /// Claim that would have been retired.
    pub old_claim_id: ClaimId,
    /// Proposed successor claim.
    pub new_claim_id: ClaimId,
    /// Typed reason no authority-bearing event was appended.
    pub reason: ExplicitSupersessionHoldReason,
}

/// Applied and held outcomes from the shared explicit-replacement policy.
pub(crate) struct ExplicitSupersessionOutcome {
    /// Authority-bearing supersession events safe to append.
    pub applied: Vec<ClaimSupersededV2>,
    /// Proposals withheld pending authoritative source order.
    pub held: Vec<HeldExplicitSupersession>,
}

pub(super) struct IngestExecution {
    source_count: u32,
    claim_count: u32,
    supersede_count: u32,
    held_supersessions: Vec<HeldExplicitSupersession>,
    append_ms: u64,
    project_ms: u64,
}

pub(super) fn execute_ingest_plan(
    cx: &mut syncbat::Ctx<'_>,
    plan: &SourcePlan,
    config: &crate::config::WorkspaceConfig,
    view: &mut std::sync::Arc<WorkspaceView>,
    observed_at_ms: u64,
    dry_run: bool,
    mut project_ms: u64,
) -> Result<IngestExecution, TexoError> {
    if dry_run {
        let source_count = u32::try_from(plan.sources.len()).unwrap_or(u32::MAX);
        let claim_count = plan.sources.iter().fold(0_u32, |count, source| {
            count.saturating_add(u32::try_from(source.claims.len()).unwrap_or(u32::MAX))
        });
        return Ok(IngestExecution {
            source_count,
            claim_count,
            supersede_count: 0,
            held_supersessions: Vec::new(),
            append_ms: 0,
            project_ms,
        });
    }
    let append_started = Instant::now();
    let (source_count, claim_count) = append_planned_sources(cx, &plan.sources)?;
    let mut append_ms = elapsed_ms(append_started);
    let project_started = Instant::now();
    *view = assemble_current_view()?;
    project_ms = project_ms.saturating_add(elapsed_ms(project_started));
    let (supersede_count, held_supersessions, inference_ms) =
        infer_and_append_ingest_supersessions(cx, plan, config, view, observed_at_ms)?;
    append_ms = append_ms.saturating_add(inference_ms);
    Ok(IngestExecution {
        source_count,
        claim_count,
        supersede_count,
        held_supersessions,
        append_ms,
        project_ms,
    })
}

fn append_planned_sources(
    cx: &mut syncbat::Ctx<'_>,
    sources: &[PlannedSource],
) -> Result<(u32, u32), TexoError> {
    let mut source_count = 0_u32;
    let mut claim_count = 0_u32;
    for source in sources {
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
    Ok((source_count, claim_count))
}

fn infer_and_append_ingest_supersessions(
    cx: &mut syncbat::Ctx<'_>,
    plan: &SourcePlan,
    config: &crate::config::WorkspaceConfig,
    view: &WorkspaceView,
    observed_at_ms: u64,
) -> Result<(u32, Vec<HeldExplicitSupersession>, u64), TexoError> {
    if config
        .semantics
        .as_ref()
        .is_some_and(|semantics| semantics.enabled)
    {
        return Ok((0, Vec::new(), 0));
    }
    let new_claims = plan
        .sources
        .iter()
        .flat_map(|source| source.claims.iter().cloned())
        .collect::<Vec<_>>();
    let temporal = workspace_temporal_policy(view)?;
    let inference = infer_supersessions(view, &new_claims, observed_at_ms, &temporal)?;
    let append_started = Instant::now();
    let mut count = 0_u32;
    for superseded in inference.applied {
        append_json(
            "texo.ingest.run",
            cx,
            <ClaimSupersededV2 as EventPayload>::KIND,
            &superseded,
        )?;
        count = count.saturating_add(1);
    }
    Ok((count, inference.held, elapsed_ms(append_started)))
}

pub(super) fn finish_ingest_output(
    root: &Path,
    workspace_id: String,
    plan: SourcePlan,
    input: &IngestRunInput,
    execution: IngestExecution,
) -> Result<IngestRunOutput, TexoError> {
    let outcome = if plan.skipped.is_empty() {
        IngestCompletion::Complete
    } else {
        IngestCompletion::Partial
    };
    let skipped_total = plan.skipped.len();
    let (skipped, skipped_artifact) =
        settle_source_failures(root, input.observed_at_ms, plan.skipped)?;
    let events_appended = if input.dry_run {
        0
    } else {
        u64::from(execution.source_count)
            .saturating_add(u64::from(execution.claim_count))
            .saturating_add(u64::from(execution.supersede_count))
    };
    Ok(IngestRunOutput {
        outcome,
        workspace_id,
        sources_observed: execution.source_count,
        claims_recorded: execution.claim_count,
        claims_superseded: execution.supersede_count,
        supersessions_held: execution.held_supersessions.len(),
        held_supersessions: execution.held_supersessions,
        dry_run: input.dry_run,
        empty: plan.empty,
        skipped,
        skipped_total,
        skipped_artifact,
        phase_ms: IngestPhaseMs {
            discover: plan.discover_ms,
            extract: plan.extract_ms,
            append: execution.append_ms,
            project: execution.project_ms,
        },
        events_appended,
        receipts: if input.dry_run {
            Vec::new()
        } else {
            take_receipts()?
        },
    })
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
/// write workspace configuration selects code executed by the extractor. Texo
/// stages only the selected input and runs the command through the fail-closed
/// bvisor adapter; there is no unconfined fallback.
fn extract_cmd_claims(
    op: &str,
    _root: &Path,
    cmd: &str,
    path: &Path,
    doc: &MarkdownDocument,
    workspace_id: &str,
    observed_at_ms: u64,
) -> Result<Vec<ClaimRecordedV2>, TexoError> {
    let output =
        crate::compat::bvisor::run_extractor(cmd, path).map_err(|error| TexoError::Extract {
            detail: format!("{op}: confined extractor failed: {error}"),
        })?;
    let stdout = String::from_utf8(output).map_err(|error| TexoError::Extract {
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
