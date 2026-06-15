//! Static claim explorer HTML.

use std::string::ToString;

use crate::agent::context::AgentContext;
use crate::stale::diagnostic::StalenessReport;
use crate::state::conflict_lifecycle::ConflictReport;

/// Render static explorer index.html.
pub fn render_index_html(
    context: &AgentContext,
    stale: &StalenessReport,
    conflicts: &ConflictReport,
) -> String {
    let mut claim_cards = String::new();
    for claim in &context.claims {
        let supersedes = if claim.supersedes.is_empty() {
            String::new()
        } else {
            format!(
                "<p><strong>supersedes:</strong> {}</p>",
                claim
                    .supersedes
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        claim_cards.push_str(&format!(
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
            subject = claim.subject_hint,
            seq = claim.receipt.sequence,
            frontier = context.replayed_through_sequence,
            path = claim.source.path,
            line = claim.source.line_start,
            receipt = claim.receipt.event_id,
            supersedes = supersedes,
            text = html_escape(&claim.text),
        ));
    }

    let mut stale_cards = String::new();
    for diag in &stale.diagnostics {
        stale_cards.push_str(&format!(
            r#"<article class="claim-card stale">
  <h2>Stale line {}:{}</h2>
  <p>{msg}</p>
</article>"#,
            diag.file,
            diag.line_start,
            msg = html_escape(&diag.message)
        ));
    }

    format!(
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
    <p>Every claim below was replayed from a BatPak journal. \
       The generated onboarding doc is a projection, not source truth.</p>
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
    texo uses one local BatPak journal. Sequences are per-store. \
    No global order, network consensus, or distributed replication is claimed.
  </footer>
</body>
</html>"#,
        claim_cards = claim_cards,
        stale_cards = stale_cards,
        conflict_count = conflicts.conflicts.len(),
        conflicts_json =
            html_escape(&serde_json::to_string_pretty(conflicts).unwrap_or_else(|_| "{}".into())),
    )
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Compile output bundle paths.
#[derive(Debug, Clone)]
pub struct CompileOutput {
    /// Generated files as (relative path, content).
    pub files: Vec<(String, String)>,
}

/// Build all compile artifacts.
pub fn compile_artifacts(
    context: &AgentContext,
    state: &crate::replay::state::ClaimState,
    staleness_report: &StalenessReport,
    conflicts: &ConflictReport,
) -> Result<CompileOutput, serde_json::Error> {
    let mut files = Vec::new();
    files.push((
        "onboarding.generated.md".to_string(),
        crate::render::markdown::render_onboarding(context),
    ));
    files.push((
        "claims.json".to_string(),
        crate::render::json::render_claims_json(state)?,
    ));
    files.push((
        "stale-context.json".to_string(),
        crate::render::json::render_stale_json(staleness_report)?,
    ));
    files.push((
        "conflicts.json".to_string(),
        crate::render::json::render_conflicts_json(conflicts)?,
    ));
    files.push((
        "agent-context.json".to_string(),
        crate::render::json::render_agent_json(context)?,
    ));
    files.push((
        "index.html".to_string(),
        render_index_html(context, staleness_report, conflicts),
    ));
    Ok(CompileOutput { files })
}
