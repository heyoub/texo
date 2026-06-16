//! Replay projection state.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types::ids::{ClaimId, ConflictId, SourceId};
use crate::types::receipt::ReceiptView;
use crate::types::sequence::LocalSequence;
use crate::types::status::{ClaimStatus, ConflictStatus};

/// Source document view in replayed state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceView {
    /// Source id.
    pub source_id: SourceId,
    /// Workspace id.
    pub workspace_id: String,
    /// Source kind.
    pub source_kind: String,
    /// Path.
    pub path: String,
    /// Body hash hex.
    pub body_hash_hex: String,
    /// Observation timestamp.
    pub observed_at_ms: u64,
    /// Receipt for the observation event.
    pub receipt: ReceiptView,
}

/// Claim view in replayed state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimView {
    /// Claim id.
    pub claim_id: ClaimId,
    /// Workspace id.
    pub workspace_id: String,
    /// Source id.
    pub source_id: SourceId,
    /// Source path.
    pub source_path: String,
    /// Start line.
    pub line_start: u32,
    /// End line.
    pub line_end: u32,
    /// Raw text.
    pub text: String,
    /// Normalized text.
    pub normalized_text: String,
    /// Subject hint.
    pub subject_hint: String,
    /// Predicate hint.
    pub predicate_hint: String,
    /// Object hint.
    pub object_hint: String,
    /// Confidence ppm.
    pub confidence_ppm: u32,
    /// Extractor kind.
    pub extractor_kind: String,
    /// Lifecycle status.
    pub status: ClaimStatus,
    /// Receipt for claim recorded event.
    pub receipt: ReceiptView,
    /// Claim ids this claim supersedes (as new claim).
    pub supersedes: Vec<ClaimId>,
    /// If superseded, the replacing claim id.
    pub superseded_by: Option<ClaimId>,
}

/// Supersession view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupersessionView {
    /// Old claim id.
    pub old_claim_id: ClaimId,
    /// New claim id.
    pub new_claim_id: ClaimId,
    /// Reason.
    pub reason: String,
    /// Decided by.
    pub decided_by: String,
    /// Receipt.
    pub receipt: ReceiptView,
}

/// Conflict view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConflictView {
    /// Conflict id.
    pub conflict_id: ConflictId,
    /// Claim a.
    pub claim_a: ClaimId,
    /// Claim b.
    pub claim_b: ClaimId,
    /// Reason.
    pub reason: String,
    /// Status.
    pub status: ConflictStatus,
    /// Receipt.
    pub receipt: ReceiptView,
}

/// Full replayed claim-chain state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimState {
    /// Observed sources by id.
    pub sources: HashMap<SourceId, SourceView>,
    /// Claims by id.
    pub claims: HashMap<ClaimId, ClaimView>,
    /// Supersession records by old claim id.
    pub superseded: HashMap<ClaimId, SupersessionView>,
    /// Conflicts by id.
    pub conflicts: HashMap<ConflictId, ConflictView>,
    /// Current claim ids grouped by subject hint.
    pub current_by_subject: HashMap<String, Vec<ClaimId>>,
    /// Maximum local sequence replayed.
    pub replayed_through_sequence: u64,
}

impl ClaimState {
    /// Lookup claim by id.
    pub fn claim(&self, id: &ClaimId) -> Option<&ClaimView> {
        self.claims.get(id)
    }

    /// Current claims optionally filtered by subject.
    pub fn current_claims(&self, subject: Option<&str>) -> Vec<&ClaimView> {
        self.claims
            .values()
            .filter(|c| c.status == ClaimStatus::Current)
            .filter(|c| subject.is_none_or(|s| c.subject_hint == s))
            .collect()
    }

    /// Rebuild subject index from claim statuses.
    ///
    /// `self.claims` is a `HashMap`, so iterating its values yields claim ids in a
    /// process-randomized order. The per-subject id vectors are sorted afterwards
    /// so the index is a deterministic function of the claim set — folding the
    /// same events twice produces an identical `ClaimState` (replay determinism),
    /// independent of HashMap iteration order.
    pub fn rebuild_subject_index(&mut self) {
        self.current_by_subject.clear();
        for claim in self.claims.values() {
            if claim.status == ClaimStatus::Current {
                self.current_by_subject
                    .entry(claim.subject_hint.clone())
                    .or_default()
                    .push(claim.claim_id.clone());
            }
        }
        for ids in self.current_by_subject.values_mut() {
            ids.sort();
        }
    }

    /// Advance replay frontier.
    pub fn note_sequence(&mut self, sequence: LocalSequence) {
        self.replayed_through_sequence = self.replayed_through_sequence.max(sequence.get());
    }
}
