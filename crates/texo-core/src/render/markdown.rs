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

    if !context.conflicts.is_empty() {
        out.push_str("\n## Conflicts (unresolved — both claimed, neither wins)\n\n");
        for conflict in &context.conflicts {
            out.push_str(&format!(
                "- \"{}\" ({}) vs \"{}\" ({})\n",
                conflict.claim_a_text, conflict.claim_a, conflict.claim_b_text, conflict.claim_b
            ));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context::{
        AgentClaim, AgentConflict, AgentReceipt, AgentSource, AgentStaleClaim,
    };
    use crate::agent::freshness::FreshnessView;
    use crate::types::ids::{ClaimId, ConflictId, WorkspaceId};
    use crate::types::status::ClaimStatus;

    fn agent_claim(id: &str) -> AgentClaim {
        AgentClaim {
            claim_id: ClaimId::try_from(id).expect("id"),
            status: ClaimStatus::Current,
            subject_hint: "deploy-process".to_string(),
            text: "Deploys happen on Friday.".to_string(),
            source: AgentSource {
                source_id: "src_abc123def456".to_string(),
                path: "policy.md".to_string(),
                line_start: 1,
            },
            receipt: AgentReceipt {
                event_id: "0x1".to_string(),
                sequence: 1,
            },
            supersedes: Vec::new(),
        }
    }

    #[test]
    fn renders_current_claims_and_stale_section() {
        // A context carrying both current AND stale claims must emit the current
        // claim bullet and a separate "Stale claims" section naming the
        // superseder — exercising the non-empty stale-claims branch.
        let context = AgentContext {
            workspace_id: WorkspaceId::new("demo").expect("ws"),
            replayed_through_sequence: 7,
            freshness: FreshnessView::batpak_local(7),
            claims: vec![agent_claim("claim_aaaaaaaaaaaa")],
            stale_claims: vec![AgentStaleClaim {
                claim_id: ClaimId::try_from("claim_bbbbbbbbbbbb").expect("id"),
                text: "Deploys happen on Monday.".to_string(),
                superseded_by: ClaimId::try_from("claim_aaaaaaaaaaaa").expect("id"),
            }],
            conflicts: Vec::new(),
        };
        let md = render_onboarding(&context);
        assert!(md.contains("sequence 7"), "must report the frontier: {md}");
        assert!(md.contains("claim_aaaaaaaaaaaa"), "current claim listed");
        assert!(md.contains("## Stale claims (do not trust)"));
        assert!(
            md.contains("claim_bbbbbbbbbbbb") && md.contains("superseded by claim_aaaaaaaaaaaa")
        );
    }

    #[test]
    fn omits_stale_section_when_no_stale_claims() {
        // With no stale claims the stale section must be absent (the
        // is_empty()-guarded branch is skipped).
        let context = AgentContext {
            workspace_id: WorkspaceId::new("demo").expect("ws"),
            replayed_through_sequence: 1,
            freshness: FreshnessView::batpak_local(1),
            claims: vec![agent_claim("claim_aaaaaaaaaaaa")],
            stale_claims: Vec::new(),
            conflicts: Vec::new(),
        };
        let md = render_onboarding(&context);
        // With no stale claims the stale section is absent entirely.
        assert!(!md.contains("Stale claims"));
        // No conflicts -> no conflicts section either.
        assert!(!md.contains("## Conflicts"));
    }

    #[test]
    fn renders_conflict_section_with_both_sides() {
        // A populated conflicts list must surface a Conflicts section naming both
        // claims — a conflicting claim is neither Current nor Stale, so without
        // this it would vanish from the projection.
        let context = AgentContext {
            workspace_id: WorkspaceId::new("demo").expect("ws"),
            replayed_through_sequence: 9,
            freshness: FreshnessView::batpak_local(9),
            claims: vec![agent_claim("claim_aaaaaaaaaaaa")],
            stale_claims: Vec::new(),
            conflicts: vec![AgentConflict {
                conflict_id: ConflictId::try_from("conflict_4b6e33f212ec").expect("cid"),
                claim_a: ClaimId::try_from("claim_bbbbbbbbbbbb").expect("id"),
                claim_a_text: "Releases happen on Monday.".to_string(),
                claim_b: ClaimId::try_from("claim_cccccccccccc").expect("id"),
                claim_b_text: "Releases go out on Friday.".to_string(),
                reason: "contradictory current claims".to_string(),
            }],
        };
        let md = render_onboarding(&context);
        assert!(
            md.contains("## Conflicts (unresolved"),
            "conflict section present"
        );
        assert!(md.contains("Releases happen on Monday."));
        assert!(md.contains("Releases go out on Friday."));
    }
}
