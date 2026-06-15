//! BatPak event payload definitions for texo (category 0xE).

use batpak::prelude::EventPayload;
use serde::{Deserialize, Serialize};

/// Record that a source document was observed and hashed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, EventPayload)]
#[batpak(category = 0xE, type_id = 1)]
pub struct SourceObserved {
    /// Deterministic source identifier.
    pub source_id: String,
    /// Workspace identifier.
    pub workspace_id: String,
    /// Source kind (markdown in v0).
    pub source_kind: String,
    /// Relative path to the source file.
    pub path: String,
    /// BLAKE3 hex hash of source body bytes.
    pub body_hash_hex: String,
    /// Observation timestamp in milliseconds.
    pub observed_at_ms: u64,
}

/// Record a claim extracted from a source line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, EventPayload)]
#[batpak(category = 0xE, type_id = 2)]
pub struct ClaimRecorded {
    /// Deterministic claim identifier.
    pub claim_id: String,
    /// Workspace identifier.
    pub workspace_id: String,
    /// Source identifier.
    pub source_id: String,
    /// Source path at observation time.
    pub source_path: String,
    /// Start line (1-based).
    pub line_start: u32,
    /// End line (1-based).
    pub line_end: u32,
    /// Raw claim text.
    pub text: String,
    /// Normalized claim text.
    pub normalized_text: String,
    /// Subject hint for grouping.
    pub subject_hint: String,
    /// Predicate hint.
    pub predicate_hint: String,
    /// Object hint.
    pub object_hint: String,
    /// Confidence in parts per million.
    pub confidence_ppm: u32,
    /// Extractor kind label.
    pub extractor_kind: String,
    /// Observation timestamp in milliseconds.
    pub observed_at_ms: u64,
}

/// Record that one claim supersedes another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, EventPayload)]
#[batpak(category = 0xE, type_id = 3)]
pub struct ClaimSuperseded {
    /// Superseded claim id.
    pub old_claim_id: String,
    /// Superseding claim id.
    pub new_claim_id: String,
    /// Workspace identifier.
    pub workspace_id: String,
    /// Human-readable reason.
    pub reason: String,
    /// Actor that decided supersession.
    pub decided_by: String,
    /// Decision timestamp in milliseconds.
    pub observed_at_ms: u64,
}

/// Record or report a conflict between two claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, EventPayload)]
#[batpak(category = 0xE, type_id = 4)]
pub struct ClaimConflictDetected {
    /// Deterministic conflict identifier.
    pub conflict_id: String,
    /// Workspace identifier.
    pub workspace_id: String,
    /// First claim id.
    pub claim_a: String,
    /// Second claim id.
    pub claim_b: String,
    /// Conflict reason.
    pub reason: String,
    /// Status string: open | resolved | ignored.
    pub status: String,
    /// Detection timestamp in milliseconds.
    pub observed_at_ms: u64,
}

/// Record compilation of a human-readable onboarding projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, EventPayload)]
#[batpak(category = 0xE, type_id = 5)]
pub struct OnboardingCompiled {
    /// Compiled document id.
    pub doc_id: String,
    /// Workspace identifier.
    pub workspace_id: String,
    /// Output path relative to workspace.
    pub output_path: String,
    /// Claim ids included in the projection.
    pub source_claim_ids: Vec<String>,
    /// Replay frontier at compile time.
    pub replayed_through_sequence: u64,
    /// Compile timestamp in milliseconds.
    pub compiled_at_ms: u64,
}
