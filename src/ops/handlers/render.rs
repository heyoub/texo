use super::agent_context::AgentContextOutput;
use super::common::{
    assemble_snapshot_view, claim_receipt, op_runtime, parse_input, run_op,
    WORKSPACE_VIEW_PROJECTION,
};
use super::ingest::resolve_path;
use super::model::AgentReceiptRow;
use crate::claims::workspace::WorkspaceView;
use crate::error::TexoError;
use crate::events::ids::SourceId;
use crate::extract::markdown::{collect_markdown_files, MarkdownDocument};
use crate::extract::normalize::normalize_line;
use crate::knowledge::SnapshotRead;
use crate::ops::env;
use crate::relate::heuristic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use syncbat::HandlerResult;

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
#[derive(Debug, Deserialize)]
struct StalenessCheckInput {
    path: PathBuf,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct StalenessReport {
    workspace_id: String,
    checked_path: String,
    replayed_through_sequence: u64,
    diagnostics: Vec<StaleDiagnostic>,
    snapshot: SnapshotRead,
}

impl StalenessReport {
    pub(super) fn empty(
        workspace_id: String,
        replayed_through_sequence: u64,
        snapshot: SnapshotRead,
    ) -> Self {
        Self {
            workspace_id,
            checked_path: ".".to_string(),
            replayed_through_sequence,
            diagnostics: Vec::new(),
            snapshot,
        }
    }
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

pub(super) fn render_onboarding(context: &AgentContextOutput) -> String {
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

pub(super) fn render_index_html(
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
    for diagnostic in &stale.diagnostics {
        write!(
            &mut stale_cards,
            r#"<article class="claim-card stale">
  <h2>Stale line {}:{}</h2>
  <p>{}</p>
</article>"#,
            diagnostic.file,
            diagnostic.line_start,
            html_escape(&diagnostic.message)
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

pub(super) fn check_staleness_from_view(
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
        diagnostics.extend(stale_diagnostics_for_path(view, &by_id, root, &path)?);
    }
    Ok(StalenessReport {
        workspace_id: workspace_id.to_string(),
        checked_path,
        replayed_through_sequence: view.frontier,
        diagnostics,
        snapshot,
    })
}

fn stale_diagnostics_for_path(
    view: &WorkspaceView,
    by_id: &BTreeMap<String, &crate::claims::workspace::ClaimView>,
    root: &Path,
    path: &Path,
) -> Result<Vec<StaleDiagnostic>, TexoError> {
    let mut diagnostics = Vec::new();

    let doc = MarkdownDocument::from_path(path, root).map_err(|error| TexoError::Source {
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

    Ok(diagnostics)
}
