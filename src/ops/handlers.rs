//! First texo operation handlers.
#![expect(
    missing_docs,
    reason = "syncbat::operation generates public registration shims without doc injection hooks"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use batpak::event::{EventKind, EventPayload};
use batpak::id::EntityIdType;
use batpak::store::Freshness;
use serde::{Deserialize, Serialize};
use syncbat::{CoreBuilder, HandlerError, HandlerResult, OperationRegisterItem};

use crate::claims::card::ClaimCard;
use crate::claims::timeline::{ClaimTimeline, TimelineEntry};
use crate::claims::workspace::{assemble, ClaimView, WorkspaceView};
use crate::config::{TexoRootConfig, WorkspaceEntry};
use crate::error::TexoError;
use crate::events::coordinate::{entity_for_claim, scope_for_workspace};
use crate::events::ids::{claim_id_from_parts, SourceId};
use crate::events::machines::{
    transition_record, TransitionCauseV1, CLAIM_EDGES, CLAIM_MACHINE, CONFLICT_EDGES,
    CONFLICT_MACHINE,
};
use crate::events::payloads::{
    ClaimRecordedV2, ClaimSupersededV2, ConflictOpenedV2, ConflictResolvedV2, OnboardingCompiledV2,
    SessionTurnV1, SourceObservedV2, WorkspaceInitializedV2,
};
use crate::extract::hints::hints_from_line;
use crate::extract::markdown::{collect_markdown_files, MarkdownDocument};
use crate::extract::normalize::normalize_line;
use crate::ops::env::{self, ReceiptNote};

const WORKSPACE_VIEW_PROJECTION: &str = "texo.workspace.view.v2";
const CLAIM_EXPLAIN_PROJECTION: &str = "texo.claim.explain.v2";

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
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&config_path, raw.as_bytes())?;
        let config_digest_hex = blake3::hash(raw.as_bytes()).to_hex().to_string();

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
        let mut view = assemble_current_view()?;
        let path = resolve_path(&root, &input.path);
        let sources = plan_sources(
            "texo.ingest.run",
            &root,
            &path,
            &workspace_id,
            input.observed_at_ms,
            config.extractor_cmd.as_deref(),
            &view,
        )?;

        let mut source_count = 0_u32;
        let mut claim_count = 0_u32;
        let mut supersede_count = 0_u32;

        if input.dry_run {
            for source in &sources {
                source_count = source_count.saturating_add(1);
                let planned = u32::try_from(source.claims.len()).unwrap_or(u32::MAX);
                claim_count = claim_count.saturating_add(planned);
            }
        } else {
            for source in &sources {
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

            view = assemble_current_view()?;
            let new_claims = sources
                .iter()
                .flat_map(|source| source.claims.iter().cloned())
                .collect::<Vec<_>>();
            for superseded in infer_supersessions(&view, &new_claims, input.observed_at_ms) {
                append_json(
                    "texo.ingest.run",
                    cx,
                    <ClaimSupersededV2 as EventPayload>::KIND,
                    &superseded,
                )?;
                supersede_count = supersede_count.saturating_add(1);
            }
        }

        Ok(IngestRunOutput {
            workspace_id,
            sources_observed: source_count,
            claims_recorded: claim_count,
            claims_superseded: supersede_count,
            dry_run: input.dry_run,
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
    input_schema = "texo.claims.list.input.v2",
    output_schema = "texo.claims.list.output.v2",
    receipt_kind = "receipt.texo.claims.list.v2",
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn claims_list(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.claims.list", || {
        let input: ClaimsListInput = parse_input("texo.claims.list", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.claims.list", error))?;
        let view = assemble_current_view()?;
        let claims = claim_list_rows(&view, input.subject.as_deref())?;
        Ok(ClaimsListOutput {
            workspace_id: view.workspace_id,
            frontier: view.frontier,
            claims,
        })
    })
}

#[syncbat::operation(
    descriptor = CLAIM_EXPLAIN,
    register = register_claim_explain,
    register_item = claim_explain_item,
    name = "texo.claim.explain",
    effect = Inspect,
    input_schema = "texo.claim.explain.input.v2",
    output_schema = "texo.claim.explain.output.v2",
    receipt_kind = "receipt.texo.claim.explain.v2",
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
        env::with(|op_env| {
            if op_env.store.by_entity(&entity).is_empty() {
                return Err(TexoError::MissingEntity {
                    entity: entity.clone(),
                });
            }
            let (card, timeline) = op_env
                .store
                .project_fused2::<ClaimCard, ClaimTimeline>(&entity)?;
            let card = card.ok_or_else(|| TexoError::MissingEntity {
                entity: entity.clone(),
            })?;
            Ok(ClaimExplainOutput {
                card,
                timeline: timeline.map_or_else(Vec::new, |timeline| timeline.entries),
            })
        })?
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
        if old_card.phase != 1 {
            return Err(TexoError::Transition {
                machine: CLAIM_MACHINE.to_string(),
                from: old_card.phase,
                to: 2,
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
            receipt: take_one_receipt("texo.claim.supersede")?,
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
    reads_events = ["evt.e001", "evt.e002", "evt.e003", "evt.e004", "evt.e005", "evt.e006", "evt.e007", "evt.e008"],
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn verify_run(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.verify.run", || {
        let _input: VerifyRunInput = parse_input("texo.verify.run", input)?;
        for kind in DOMAIN_KINDS {
            cx.event_read_handle()
                .read_event(format!("evt.{:04x}", kind.as_raw_u16()))
                .map_err(|error| op_runtime("texo.verify.run", error))?;
        }
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.verify.run", error))?;

        let mut errors = Vec::new();
        let (journal_ok, view) = env::with(|op_env| {
            let chain = op_env.store.verify_chain()?;
            let mut journal_ok = chain.is_intact();
            if !chain.is_intact() {
                errors.push(format!("chain: {chain:?}"));
            }
            let scope = scope_for_workspace(&op_env.workspace_id);
            for entry in op_env.store.by_scope(&scope) {
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
            let mut cache = op_env.cache.borrow_mut();
            let view = assemble(&op_env.store, &op_env.workspace_id, &mut cache)?;
            Ok::<_, TexoError>((journal_ok, view))
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
        })
    })
}

const DOMAIN_KINDS: [EventKind; 8] = [
    <SourceObservedV2 as EventPayload>::KIND,
    <ClaimRecordedV2 as EventPayload>::KIND,
    <ClaimSupersededV2 as EventPayload>::KIND,
    <ConflictOpenedV2 as EventPayload>::KIND,
    <OnboardingCompiledV2 as EventPayload>::KIND,
    <ConflictResolvedV2 as EventPayload>::KIND,
    <WorkspaceInitializedV2 as EventPayload>::KIND,
    <SessionTurnV1 as EventPayload>::KIND,
];

/// Return the six operation registration items.
#[must_use]
pub fn catalog() -> Vec<OperationRegisterItem> {
    vec![
        workspace_init_item(),
        ingest_run_item(),
        claims_list_item(),
        claim_explain_item(),
        claim_supersede_item(),
        verify_run_item(),
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
    register_claim_explain(builder)?;
    register_claim_supersede(builder)?;
    register_verify_run(builder)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct WorkspaceInitInput {
    workspace_id: String,
}

#[derive(Debug, Serialize)]
struct WorkspaceInitOutput {
    workspace_id: String,
    config_path: String,
    receipt: ReceiptNote,
}

#[derive(Debug, Deserialize)]
struct IngestRunInput {
    path: PathBuf,
    dry_run: bool,
    observed_at_ms: u64,
}

#[derive(Debug, Serialize)]
struct IngestRunOutput {
    workspace_id: String,
    sources_observed: u32,
    claims_recorded: u32,
    claims_superseded: u32,
    dry_run: bool,
    receipts: Vec<ReceiptNote>,
}

#[derive(Debug, Deserialize)]
struct ClaimsListInput {
    subject: Option<String>,
}

#[derive(Debug, Serialize)]
struct ClaimsListOutput {
    workspace_id: String,
    frontier: u64,
    claims: Vec<AgentClaimRow>,
}

#[derive(Debug, Deserialize)]
struct ClaimExplainInput {
    claim_id: String,
}

#[derive(Debug, Serialize)]
struct ClaimExplainOutput {
    card: ClaimCard,
    timeline: Vec<TimelineEntry>,
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
    receipt: ReceiptNote,
}

#[derive(Debug, Deserialize)]
struct VerifyRunInput {}

#[derive(Debug, Serialize)]
struct VerifyRunOutput {
    projection_ok: bool,
    journal_ok: bool,
    transitions_ok: bool,
    errors: Vec<String>,
}

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

#[derive(Debug, Serialize)]
struct AgentReceiptRow {
    event_id: String,
    sequence: u64,
}

struct PlannedSource {
    observed: SourceObservedV2,
    claims: Vec<ClaimRecordedV2>,
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

fn run_op<T: Serialize>(
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

fn parse_input<T: serde::de::DeserializeOwned>(op: &str, input: &[u8]) -> Result<T, TexoError> {
    serde_json::from_slice(input).map_err(|error| TexoError::OpInput {
        op: op.to_string(),
        detail: error.to_string(),
    })
}

fn append_json<T: Serialize>(
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

fn take_receipts() -> Result<Vec<ReceiptNote>, TexoError> {
    env::with(|op_env| op_env.receipts.borrow_mut().drain(..).collect())
}

fn take_one_receipt(op: &str) -> Result<ReceiptNote, TexoError> {
    let mut receipts = take_receipts()?;
    receipts.pop().ok_or_else(|| TexoError::OpRuntime {
        op: op.to_string(),
        detail: "append produced no receipt".to_string(),
        denied: false,
    })
}

fn op_runtime(op: &str, error: impl std::fmt::Display) -> TexoError {
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

fn assemble_current_view() -> Result<WorkspaceView, TexoError> {
    env::with(|op_env| {
        let mut cache = op_env.cache.borrow_mut();
        assemble(&op_env.store, &op_env.workspace_id, &mut cache)
    })?
}

fn resolve_path(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn plan_sources(
    op: &str,
    root: &Path,
    input_path: &Path,
    workspace_id: &str,
    observed_at_ms: u64,
    extractor_cmd: Option<&str>,
    view: &WorkspaceView,
) -> Result<Vec<PlannedSource>, TexoError> {
    let existing_hashes = view
        .sources
        .iter()
        .map(|source| source.body_hash_hex.clone())
        .collect::<BTreeSet<_>>();
    let mut batch_hashes = BTreeSet::new();
    let mut planned = Vec::new();
    let files = collect_markdown_files(input_path).map_err(|error| TexoError::Source {
        path: input_path.to_string_lossy().to_string(),
        detail: error.to_string(),
    })?;
    for path in files {
        let doc = MarkdownDocument::from_path(&path, root).map_err(|error| TexoError::Source {
            path: path.to_string_lossy().to_string(),
            detail: error.to_string(),
        })?;
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
    Ok(planned)
}

fn extract_heuristic_claims(
    doc: &MarkdownDocument,
    workspace_id: &str,
    observed_at_ms: u64,
) -> Result<Vec<ClaimRecordedV2>, TexoError> {
    let source_id = SourceId::try_from(doc.source_id.as_str())?;
    let mut claims = Vec::new();
    for line in &doc.lines {
        let Some(hints) = hints_from_line(&line.text) else {
            continue;
        };
        let normalized = normalize_line(&line.text);
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
        .arg(cmd)
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

fn infer_supersessions(
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
    [
        "moved",
        "changed",
        "now",
        "no longer",
        "replaced",
        "instead",
        "new process",
        "as of",
        "decided",
    ]
    .iter()
    .any(|needle| normalized_text.contains(needle))
}

fn claim_list_rows(
    view: &WorkspaceView,
    subject: Option<&str>,
) -> Result<Vec<AgentClaimRow>, TexoError> {
    let mut rows = Vec::new();
    for claim in &view.claims {
        if subject.is_some_and(|wanted| claim.card.subject_hint.as_deref() != Some(wanted)) {
            continue;
        }
        let receipt = claim_receipt(&claim.card.claim_id)?;
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
