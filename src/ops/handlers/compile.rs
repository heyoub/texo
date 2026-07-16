use super::agent_context::{build_agent_context_from_view, AgentContextOutput};
use super::common::{
    append_json, assemble_current_view, op_runtime, parse_input, run_op, snapshot_for_view,
    take_one_receipt, WORKSPACE_VIEW_PROJECTION,
};
use super::ingest::resolve_path;
use super::relate::require_complete_settlement;
use super::render::{self, StalenessReport};
use crate::claims::workspace::WorkspaceView;
use crate::error::TexoError;
use crate::events::payloads::OnboardingCompiledV2;
use crate::ops::env;
use crate::ops::env::ReceiptNote;
use crate::relate::heuristic;
use batpak::event::EventPayload;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use syncbat::HandlerResult;

#[syncbat::operation(
    descriptor = COMPILE_RUN,
    register = register_compile_run,
    register_item = compile_run_item,
    name = "texo.compile.run",
    effect = Persist,
    input_schema = "texo.compile.run.input.v3",
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
        let view = assemble_current_view()?;
        if !input.allow_unsettled {
            require_complete_settlement(&view)?;
        }
        let snapshot = snapshot_for_view(&view)?;
        let context = build_agent_context_from_view(&view, None, true, snapshot.clone())?;
        let conflict_report = heuristic::detect_conflicts(&view)?;
        let (root, workspace_id) =
            env::with(|op_env| (op_env.root.clone(), op_env.workspace_id.clone()))?;
        let out_dir = resolve_path(&root, &input.out_dir);
        let stale_report = StalenessReport::empty(workspace_id.clone(), view.frontier, snapshot);
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
#[derive(Debug, Deserialize)]
struct CompileRunInput {
    out_dir: PathBuf,
    observed_at_ms: u64,
    #[serde(default)]
    allow_unsettled: bool,
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

fn compile_artifacts(
    context: &AgentContextOutput,
    view: &WorkspaceView,
    stale: &StalenessReport,
    conflicts: &heuristic::ConflictReport,
) -> Result<Vec<CompileFile>, TexoError> {
    Ok(vec![
        CompileFile {
            name: "onboarding.generated.md".to_string(),
            contents: render::render_onboarding(context),
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
            contents: render::render_index_html(context, stale, conflicts)?,
        },
    ])
}
