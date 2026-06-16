//! Agent context JSON builder.

use serde::{Deserialize, Serialize};

use crate::agent::freshness::FreshnessView;
use crate::replay::state::ClaimState;
use crate::types::ids::{ClaimId, ConflictId, WorkspaceId};
use crate::types::receipt::ReceiptView;
use crate::types::status::{ClaimStatus, ConflictStatus};

/// Source provenance in agent context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSource {
    /// Source id.
    pub source_id: String,
    /// Path.
    pub path: String,
    /// Line start.
    pub line_start: u32,
}

/// Receipt block in agent context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentReceipt {
    /// Event id hex.
    pub event_id: String,
    /// Local sequence.
    pub sequence: u64,
}

/// Current claim in agent context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentClaim {
    /// Claim id.
    pub claim_id: ClaimId,
    /// Status.
    pub status: ClaimStatus,
    /// Subject hint.
    pub subject_hint: String,
    /// Text.
    pub text: String,
    /// Source provenance.
    pub source: AgentSource,
    /// Receipt.
    pub receipt: AgentReceipt,
    /// Claim ids superseded by this claim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supersedes: Vec<ClaimId>,
}

/// Stale claim summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStaleClaim {
    /// Claim id.
    pub claim_id: ClaimId,
    /// Text.
    pub text: String,
    /// Superseded by.
    pub superseded_by: ClaimId,
}

/// An unresolved conflict between two co-current claims, with both sides' text
/// resolved for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConflict {
    /// Deterministic conflict id.
    pub conflict_id: ConflictId,
    /// First claim id.
    pub claim_a: ClaimId,
    /// First claim text.
    pub claim_a_text: String,
    /// Second claim id.
    pub claim_b: ClaimId,
    /// Second claim text.
    pub claim_b_text: String,
    /// Why the pair conflicts.
    pub reason: String,
}

/// Full agent context snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentContext {
    /// Workspace id.
    pub workspace_id: WorkspaceId,
    /// Replay frontier.
    pub replayed_through_sequence: u64,
    /// Freshness metadata.
    pub freshness: FreshnessView,
    /// Current claims.
    pub claims: Vec<AgentClaim>,
    /// Stale claims.
    pub stale_claims: Vec<AgentStaleClaim>,
    /// Unresolved conflicts. Omitted when empty so consumers without conflicts
    /// (e.g. the heuristic path) see an unchanged shape.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<AgentConflict>,
}

/// Build agent context from replayed state.
pub fn build_agent_context(
    state: &ClaimState,
    workspace_id: &WorkspaceId,
    subject_filter: Option<&str>,
) -> AgentContext {
    let mut claims = Vec::new();
    let mut stale_claims = Vec::new();

    for claim in state.claims.values() {
        if subject_filter.is_some_and(|s| claim.subject_hint != s) {
            continue;
        }
        if claim.status == ClaimStatus::Current {
            claims.push(AgentClaim {
                claim_id: claim.claim_id.clone(),
                status: claim.status,
                subject_hint: claim.subject_hint.clone(),
                text: claim.text.clone(),
                source: AgentSource {
                    source_id: claim.source_id.to_string(),
                    path: claim.source_path.clone(),
                    line_start: claim.line_start,
                },
                receipt: receipt_to_agent(&claim.receipt),
                supersedes: claim.supersedes.clone(),
            });
        } else if claim.status == ClaimStatus::Superseded {
            if let Some(by) = &claim.superseded_by {
                stale_claims.push(AgentStaleClaim {
                    claim_id: claim.claim_id.clone(),
                    text: claim.text.clone(),
                    superseded_by: by.clone(),
                });
            }
        }
    }

    // Open conflicts, with both sides' text resolved for display. A conflicting
    // claim is neither Current nor Superseded, so without this it would vanish
    // from the projection entirely.
    let mut conflicts = Vec::new();
    for conflict in state.conflicts.values() {
        if conflict.status != ConflictStatus::Open {
            continue;
        }
        let text_of = |id: &ClaimId| state.claim(id).map(|c| c.text.clone()).unwrap_or_default();
        conflicts.push(AgentConflict {
            conflict_id: conflict.conflict_id.clone(),
            claim_a: conflict.claim_a.clone(),
            claim_a_text: text_of(&conflict.claim_a),
            claim_b: conflict.claim_b.clone(),
            claim_b_text: text_of(&conflict.claim_b),
            reason: conflict.reason.clone(),
        });
    }

    claims.sort_by_key(|c| c.receipt.sequence);
    stale_claims.sort_by(|a, b| a.claim_id.as_str().cmp(b.claim_id.as_str()));
    conflicts.sort_by(|a, b| a.conflict_id.as_str().cmp(b.conflict_id.as_str()));

    AgentContext {
        workspace_id: workspace_id.clone(),
        replayed_through_sequence: state.replayed_through_sequence,
        freshness: FreshnessView::batpak_local(state.replayed_through_sequence),
        claims,
        stale_claims,
        conflicts,
    }
}

fn receipt_to_agent(receipt: &ReceiptView) -> AgentReceipt {
    AgentReceipt {
        event_id: receipt.event_id.as_str().to_string(),
        sequence: receipt.sequence.get(),
    }
}

/// Explain a single claim for MCP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimExplanation {
    /// Claim id.
    pub claim_id: ClaimId,
    /// Status.
    pub status: ClaimStatus,
    /// Text.
    pub text: String,
    /// Source path and line.
    pub source: AgentSource,
    /// Receipt.
    pub receipt: AgentReceipt,
    /// Claims superseded by this one.
    pub supersedes: Vec<ClaimId>,
    /// If superseded, replacement ids.
    pub superseded_by: Vec<ClaimId>,
    /// Open conflict ids.
    pub conflicts: Vec<String>,
    /// Replay frontier.
    pub replayed_through_sequence: u64,
}

/// Build a claim explanation.
pub fn explain_claim(state: &ClaimState, claim_id: &ClaimId) -> Option<ClaimExplanation> {
    let claim = state.claim(claim_id)?;
    let superseded_by = claim.superseded_by.clone().into_iter().collect::<Vec<_>>();
    let conflicts = state
        .conflicts
        .values()
        .filter(|c| c.claim_a == *claim_id || c.claim_b == *claim_id)
        .map(|c| c.conflict_id.to_string())
        .collect();

    Some(ClaimExplanation {
        claim_id: claim.claim_id.clone(),
        status: claim.status,
        text: claim.text.clone(),
        source: AgentSource {
            source_id: claim.source_id.to_string(),
            path: claim.source_path.clone(),
            line_start: claim.line_start,
        },
        receipt: receipt_to_agent(&claim.receipt),
        supersedes: claim.supersedes.clone(),
        superseded_by,
        conflicts,
        replayed_through_sequence: state.replayed_through_sequence,
    })
}
