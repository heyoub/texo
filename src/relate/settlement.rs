//! Typed relation-settlement contracts shared by events, projections, and ops.

use serde::{Deserialize, Serialize};

use crate::events::ids::ClaimId;

/// Durable completion state for one bounded relation campaign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CampaignPhase {
    /// More candidate slots or a failed pair must be resumed.
    Partial {
        /// Exact global candidate cursor required by the next page.
        next_candidate_cursor: u64,
    },
    /// Every required candidate slot and verdict is settled.
    Complete,
}

/// Closed relation verdict recorded for a logical pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettledRelation {
    /// The newer claim replaces the older claim.
    Supersedes,
    /// The pair is an unresolved contradiction.
    Conflicts,
    /// The claims express the same fact.
    Duplicate,
    /// The claims are independent.
    Unrelated,
}

impl From<crate::semantics::ClaimRelation> for SettledRelation {
    fn from(value: crate::semantics::ClaimRelation) -> Self {
        match value {
            crate::semantics::ClaimRelation::Supersedes => Self::Supersedes,
            crate::semantics::ClaimRelation::Conflict => Self::Conflicts,
            crate::semantics::ClaimRelation::Duplicate => Self::Duplicate,
            crate::semantics::ClaimRelation::Unrelated => Self::Unrelated,
        }
    }
}

impl From<SettledRelation> for crate::semantics::ClaimRelation {
    fn from(value: SettledRelation) -> Self {
        match value {
            SettledRelation::Supersedes => Self::Supersedes,
            SettledRelation::Conflicts => Self::Conflict,
            SettledRelation::Duplicate => Self::Duplicate,
            SettledRelation::Unrelated => Self::Unrelated,
        }
    }
}

/// Closed class of a deferred pair attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationFailureClass {
    /// Positive token-limit truncation.
    Truncated,
    /// Non-success provider HTTP status.
    HttpStatus,
    /// Transport failure.
    Transport,
    /// Wall-clock request deadline.
    Deadline,
    /// Malformed or incomplete provider response.
    Parse,
    /// Global relate budget was exhausted before this pair ran.
    BudgetExhausted,
    /// Both source revisions are valid but neither descends from the other.
    TemporalConcurrent,
    /// Available source evidence cannot establish an ordering.
    TemporalUnknown,
}

/// Sanitized pair-level failure evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairFailureView {
    /// Stable failure class.
    pub class: RelationFailureClass,
    /// Provider endpoint path, when known.
    pub endpoint: Option<String>,
    /// HTTP status, when received.
    pub status: Option<u16>,
    /// Number of provider attempts.
    pub attempts: u32,
}

/// One candidate pair whose verdict remains absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedPair {
    /// Older claim id.
    pub old_claim: ClaimId,
    /// Newer claim id.
    pub new_claim: ClaimId,
    /// Older source reference (`path:line`), never claim text.
    pub old_ref: String,
    /// Newer source reference (`path:line`), never claim text.
    pub new_ref: String,
    /// Sanitized failure evidence.
    pub failure: PairFailureView,
}

/// Non-authoritative later judgment that disagrees with first-write authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityWarning {
    /// Older claim.
    pub old_claim: ClaimId,
    /// Newer claim.
    pub new_claim: ClaimId,
    /// Authoritative first verdict.
    pub prior_verdict: SettledRelation,
    /// Authoritative attempt fingerprint.
    pub prior_fingerprint: String,
    /// Later contrary verdict.
    pub new_verdict: SettledRelation,
    /// Later attempt fingerprint.
    pub new_fingerprint: String,
    /// Stable explanation.
    pub message: String,
}

/// A derived decision withheld because required pair evidence was absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HeldDecision {
    /// Supersession held because its old claim was tainted.
    Supersession {
        /// Older claim.
        old_claim: ClaimId,
        /// Proposed winner.
        new_claim: ClaimId,
        /// Deterministic derived reason.
        reason: String,
    },
    /// Conflict held because either participant was tainted.
    Conflict {
        /// Stable conflict id.
        conflict_id: crate::events::ids::ConflictId,
        /// First claim.
        claim_a: ClaimId,
        /// Second claim.
        claim_b: ClaimId,
        /// Deterministic derived reason.
        reason: String,
    },
}
