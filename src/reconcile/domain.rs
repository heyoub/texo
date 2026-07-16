use crate::events::ids::ClaimId;
use crate::knowledge::EvidenceOccurrence;

/// Bounds on candidate generation before any paid proposal call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileLimits {
    /// Maximum candidates retained for one claim.
    pub per_claim: usize,
    /// Maximum candidates retained for the complete operation.
    pub total: usize,
    /// Minimum accepted model score in parts per million.
    pub min_score_ppm: u32,
}

impl Default for ReconcileLimits {
    fn default() -> Self {
        Self {
            per_claim: 4,
            total: 256,
            min_score_ppm: 700_000,
        }
    }
}

/// Minimal semantic claim view consumed by candidate generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileClaim {
    /// Durable claim identity.
    pub claim_id: ClaimId,
    /// Exact assertion text sent to the proposal model.
    pub text: String,
    /// Optional deterministic subject hint.
    pub subject_hint: String,
    /// Optional deterministic predicate hint.
    pub predicate_hint: String,
    /// Optional deterministic object hint.
    pub object_hint: String,
}

/// One bounded claim/code pair eligible for a cached model proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileCandidate {
    /// Claim receiving evidence if policy accepts the proposal.
    pub claim_id: ClaimId,
    /// Exact semantic assertion supplied as the proposal's first assertion.
    pub claim_text: String,
    /// Exact durable occurrence constructed from the disposable code index.
    pub occurrence: EvidenceOccurrence,
    /// Role-labelled code text supplied as the proposal's second assertion.
    pub code_prompt: String,
    pub(super) rank: usize,
}
