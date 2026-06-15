//! Agent context JSON builder.

use serde::{Deserialize, Serialize};

use crate::agent::freshness::FreshnessView;
use crate::replay::state::ClaimState;
use crate::types::ids::ClaimId;
use crate::types::receipt::ReceiptView;
use crate::types::status::ClaimStatus;

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

/// Full agent context snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentContext {
    /// Workspace id.
    pub workspace_id: String,
    /// Replay frontier.
    pub replayed_through_sequence: u64,
    /// Freshness metadata.
    pub freshness: FreshnessView,
    /// Current claims.
    pub claims: Vec<AgentClaim>,
    /// Stale claims.
    pub stale_claims: Vec<AgentStaleClaim>,
}

/// Build agent context from replayed state.
pub fn build_agent_context(
    state: &ClaimState,
    workspace_id: &str,
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

    claims.sort_by_key(|c| c.receipt.sequence);
    stale_claims.sort_by(|a, b| a.claim_id.as_str().cmp(b.claim_id.as_str()));

    AgentContext {
        workspace_id: workspace_id.to_string(),
        replayed_through_sequence: state.replayed_through_sequence,
        freshness: FreshnessView::batpak_local(state.replayed_through_sequence),
        claims,
        stale_claims,
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
