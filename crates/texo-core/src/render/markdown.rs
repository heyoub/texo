//! Onboarding markdown projection.

use crate::agent::context::AgentContext;

/// Render generated onboarding markdown from agent context.
pub fn render_onboarding(context: &AgentContext) -> String {
    let mut out = String::from("# Generated Onboarding\n\n");
    out.push_str(
        "_This document is a projection replayed from the texo claim-chain. \
         It is not source truth._\n\n",
    );
    out.push_str(&format!(
        "_Replayed through local store sequence {}._\n\n",
        context.replayed_through_sequence
    ));

    out.push_str("## Current claims\n\n");
    for claim in &context.claims {
        out.push_str(&format!(
            "- **{}** ({}): {}  \n  _source: {}:{}_\n",
            claim.claim_id,
            claim.subject_hint,
            claim.text,
            claim.source.path,
            claim.source.line_start
        ));
    }

    if !context.stale_claims.is_empty() {
        out.push_str("\n## Stale claims (do not trust)\n\n");
        for stale in &context.stale_claims {
            out.push_str(&format!(
                "- {}: \"{}\" superseded by {}\n",
                stale.claim_id, stale.text, stale.superseded_by
            ));
        }
    }

    out
}
