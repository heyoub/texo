use super::common::{
    append_json, assemble_snapshot_view, config_error, op_runtime, parse_input, run_op,
    status_coverage, take_receipts, WORKSPACE_VIEW_PROJECTION,
};
use super::relate::{authoritative_settlements, settlement_is_complete};
use crate::config::{TexoRootConfig, WorkspaceEntry};
use crate::error::TexoError;
use crate::events::coordinate::entity_for_workspace_meta;
use crate::events::payloads::WorkspaceInitializedV2;
use crate::knowledge::{KnowledgeCoverage, SnapshotRead};
use crate::ops::env;
use crate::ops::env::ReceiptNote;
use batpak::event::EventPayload;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use syncbat::HandlerResult;

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
        if !root_config.workspaces.contains_key(&input.workspace_id) {
            root_config.upsert_workspace(
                &input.workspace_id,
                WorkspaceEntry::for_id(&input.workspace_id),
            );
        }

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
        let settlement_complete = settlement_is_complete(&view)?;
        let (coverage, code_index_available) = status_coverage(&view, &snapshot)?;
        let journal = env::with(|op_env| op_env.journal.clone())?;
        Ok(WorkspaceStatusOutput {
            workspace_id: view.workspace_id.clone(),
            journal_id: journal.id,
            journal_role: journal.role,
            source_journal: journal.source_journal,
            replica_mode: journal.replica_mode,
            frontier: view.frontier,
            freshness: view.freshness,
            claims_total: view.claims.len(),
            open_conflicts: view.conflicts.iter().filter(|card| card.phase == 1).count(),
            settlement_complete,
            unresolved_pairs,
            authority_warnings: settlement.warnings.len(),
            code_index_available,
            coverage,
            snapshot,
        })
    })
}
#[derive(Debug, Deserialize)]
struct WorkspaceInitInput {
    workspace_id: String,
}

#[derive(Debug, Deserialize)]
struct WorkspaceStatusInput {
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
struct WorkspaceStatusOutput {
    workspace_id: String,
    journal_id: crate::topology::JournalId,
    journal_role: crate::topology::JournalRole,
    source_journal: Option<crate::topology::JournalId>,
    replica_mode: Option<crate::topology::ReplicaMode>,
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
