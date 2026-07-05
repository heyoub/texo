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
    /// Byte offset (0-based, inclusive) of the claim's **source span** start.
    ///
    /// A claim may be a paraphrase, so claim-level offsets are ill-defined; the
    /// journaled range is the byte range of the source span the claim was
    /// extracted from ("this claim came from bytes X..Y of this doc"). Offsets
    /// that would overflow `u32` saturate at `u32::MAX` (see
    /// [`crate::extract::byte_offset_u32`]). `#[serde(default)]` keeps events
    /// journaled before v1.1 (which lack this field) decodable as `0`.
    #[serde(default)]
    pub char_start: u32,
    /// Byte offset (exclusive) of the claim's **source span** end.
    ///
    /// See [`ClaimRecorded::char_start`]; `0` when decoded from a pre-v1.1
    /// event. `char_start..char_end` slices the source body back to the span
    /// text.
    #[serde(default)]
    pub char_end: u32,
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
    /// Model identity that extracted this claim (e.g.
    /// `openrouter:anthropic/claude-opus-4.8`). Empty for extractors with no
    /// model (heuristic path) and for events journaled before v1.1
    /// (`#[serde(default)]` keeps old journals replayable).
    #[serde(default)]
    pub extractor_model: String,
    /// Version tag of the extraction prompt (e.g. `propose-v3`). Empty for
    /// extractors with no prompt (heuristic path) and for pre-v1.1 events.
    #[serde(default)]
    pub prompt_version: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A `ClaimRecorded` JSON exactly as journaled BEFORE v1.1 added
    /// `char_start`/`char_end`/`extractor_model`/`prompt_version`. There is no
    /// event-version gate: `#[serde(default)]` on the new fields IS the
    /// back-compat mechanism, so this old shape must keep decoding forever.
    const PRE_V1_1_CLAIM_RECORDED_JSON: &str = r#"{
        "claim_id": "claim_2e3b9fcf2e18",
        "workspace_id": "demo",
        "source_id": "src_abc123def456",
        "source_path": "meeting_notes.md",
        "line_start": 3,
        "line_end": 3,
        "text": "Decision: deploys moved to Tuesday.",
        "normalized_text": "decision: deploys moved to tuesday.",
        "subject_hint": "deploy-process",
        "predicate_hint": "moved",
        "object_hint": "tuesday",
        "confidence_ppm": 900000,
        "extractor_kind": "heuristic-v1",
        "observed_at_ms": 1700000000000
    }"#;

    #[test]
    fn old_format_claim_recorded_still_decodes_with_defaults() {
        // Regression: an event journaled without the v1.1 fields must decode
        // (old journals stay replayable), with the new fields defaulting.
        let payload: ClaimRecorded =
            serde_json::from_str(PRE_V1_1_CLAIM_RECORDED_JSON).expect("old event must decode");

        // Pre-existing fields decode unchanged.
        assert_eq!(payload.claim_id, "claim_2e3b9fcf2e18");
        assert_eq!(payload.line_start, 3);
        assert_eq!(payload.extractor_kind, "heuristic-v1");

        // New fields take their serde defaults.
        assert_eq!(payload.char_start, 0);
        assert_eq!(payload.char_end, 0);
        assert_eq!(payload.extractor_model, "");
        assert_eq!(payload.prompt_version, "");
    }

    #[test]
    fn new_format_claim_recorded_round_trips() {
        // A v1.1 event with the new fields populated must survive a
        // serialize/deserialize round trip bit-for-bit.
        let payload = ClaimRecorded {
            claim_id: "claim_2e3b9fcf2e18".to_string(),
            workspace_id: "demo".to_string(),
            source_id: "src_abc123def456".to_string(),
            source_path: "meeting_notes.md".to_string(),
            line_start: 3,
            line_end: 3,
            char_start: 42,
            char_end: 77,
            text: "Decision: deploys moved to Tuesday.".to_string(),
            normalized_text: "decision: deploys moved to tuesday.".to_string(),
            subject_hint: "deploy-process".to_string(),
            predicate_hint: "moved".to_string(),
            object_hint: "tuesday".to_string(),
            confidence_ppm: 900_000,
            extractor_kind: "cmd:texo-extract".to_string(),
            extractor_model: "openrouter:anthropic/claude-opus-4.8".to_string(),
            prompt_version: "propose-v3".to_string(),
            observed_at_ms: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        let back: ClaimRecorded = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, payload);
    }
}
