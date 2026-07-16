use super::agent_context::build_agent_context_from_view;
use super::common::{
    assemble_current_view, op_runtime, parse_input, run_op, snapshot_for_view,
    WORKSPACE_VIEW_PROJECTION,
};
use crate::error::TexoError;
use crate::events::coordinate::scope_for_workspace;
use crate::ops::env;
use batpak::coordinate::Region;
use serde::{Deserialize, Serialize};
use std::path::Path;
use syncbat::HandlerResult;

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
pub(super) fn stats_read(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.stats.read", || {
        let _input: StatsReadInput = parse_input("texo.stats.read", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.stats.read", error))?;
        let view = assemble_current_view()?;
        let (root, config, journal) = env::with(|op_env| {
            (
                op_env.root.clone(),
                op_env.config.clone(),
                op_env.journal.clone(),
            )
        })?;
        let store_path = config.store_path_buf(&root);
        let projection_path = root
            .join(".texo/cache/workspace-view")
            .join(format!("{}--{}.bin", view.workspace_id, journal.id));
        let context = build_agent_context_from_view(&view, None, true, snapshot_for_view(&view)?)?;
        let agent_context_bytes = serde_json::to_vec(&context)?.len();
        Ok(StatsReadOutput {
            journal_id: journal.id,
            journal_role: journal.role,
            claims_total: view.claims.len(),
            events_total: workspace_event_count()?,
            journal_bytes: journal_file_bytes(&store_path)?,
            projection_bytes: file_bytes(&projection_path)?,
            agent_context_bytes: u64::try_from(agent_context_bytes).unwrap_or(u64::MAX),
            frontier_sequence: view.frontier,
        })
    })
}
#[derive(Debug, Deserialize)]
struct StatsReadInput {}
#[derive(Debug, Serialize)]
struct StatsReadOutput {
    journal_id: crate::topology::JournalId,
    journal_role: crate::topology::JournalRole,
    claims_total: usize,
    events_total: usize,
    journal_bytes: u64,
    projection_bytes: u64,
    agent_context_bytes: u64,
    frontier_sequence: u64,
}
pub(super) fn claim_phase_name(phase: u64) -> &'static str {
    match phase {
        0 => "unrecorded",
        1 => "current",
        2 => "superseded",
        _ => "invalid",
    }
}

pub(super) fn conflict_phase_name(phase: u64) -> &'static str {
    match phase {
        0 => "unopened",
        1 => "open",
        2 => "resolved",
        3 => "ignored",
        _ => "invalid",
    }
}

pub(super) fn workspace_event_count() -> Result<usize, TexoError> {
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

pub(super) fn file_bytes(path: &Path) -> Result<u64, TexoError> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn journal_file_bytes(path: &Path) -> Result<u64, TexoError> {
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
