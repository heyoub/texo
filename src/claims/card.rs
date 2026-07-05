//! Claim card projection.

use batpak::event::RawMsgpackInput;
use serde::{Deserialize, Serialize};

use crate::events::payloads::{ClaimRecordedV2, ClaimSupersededV2};

/// Current card view for a claim entity.
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize, batpak::EventSourced)]
#[batpak(input = RawMsgpackInput, cache_version = 1, state_max_cardinality = 1)]
#[batpak(event = ClaimRecordedV2, handler = on_recorded)]
#[batpak(event = ClaimSupersededV2, handler = on_superseded)]
pub struct ClaimCard {
    /// Claim phase: 0 unrecorded, 1 current, 2 superseded.
    pub phase: u64,
    /// Stable claim identifier.
    pub claim_id: String,
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// Stable source identifier.
    pub source_id: String,
    /// Source path relative to the workspace root.
    pub source_path: String,
    /// One-based starting line.
    pub line_start: u32,
    /// One-based ending line.
    pub line_end: u32,
    /// Zero-based starting character offset.
    pub char_start: u32,
    /// Zero-based ending character offset.
    pub char_end: u32,
    /// Extracted claim text.
    pub text: String,
    /// Normalized claim text.
    pub normalized_text: String,
    /// Optional subject hint captured by extraction.
    pub subject_hint: Option<String>,
    /// Optional predicate hint captured by extraction.
    pub predicate_hint: Option<String>,
    /// Optional object hint captured by extraction.
    pub object_hint: Option<String>,
    /// Extractor confidence in parts per million.
    pub confidence_ppm: u32,
    /// Extractor implementation kind.
    pub extractor_kind: String,
    /// Extractor model identifier.
    pub extractor_model: String,
    /// Prompt version identifier.
    pub prompt_version: String,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
    /// Replacement claim identifier when superseded.
    pub superseded_by: Option<String>,
    /// Human-readable supersession reason.
    pub superseded_reason: String,
    /// Actor that made the supersession decision.
    pub superseded_decided_by: String,
    /// Domain lifecycle anomalies observed during replay.
    pub anomalies: Vec<String>,
}

impl ClaimCard {
    fn on_recorded(&mut self, event: &ClaimRecordedV2) {
        if self.phase != 0 {
            self.anomalies.push("duplicate-record".to_string());
        }
        self.phase = 1;
        self.claim_id.clone_from(&event.claim_id);
        self.workspace_id.clone_from(&event.workspace_id);
        self.source_id.clone_from(&event.source_id);
        self.source_path.clone_from(&event.source_path);
        self.line_start = event.line_start;
        self.line_end = event.line_end;
        self.char_start = event.char_start;
        self.char_end = event.char_end;
        self.text.clone_from(&event.text);
        self.normalized_text.clone_from(&event.normalized_text);
        self.subject_hint.clone_from(&event.subject_hint);
        self.predicate_hint.clone_from(&event.predicate_hint);
        self.object_hint.clone_from(&event.object_hint);
        self.confidence_ppm = event.confidence_ppm;
        self.extractor_kind.clone_from(&event.extractor_kind);
        self.extractor_model.clone_from(&event.extractor_model);
        self.prompt_version.clone_from(&event.prompt_version);
        self.observed_at_ms = event.observed_at_ms;
    }

    fn on_superseded(&mut self, event: &ClaimSupersededV2) {
        if self.phase != 1 {
            self.anomalies
                .push(format!("supersede-from-{}", self.phase));
        }
        if self.superseded_by.is_some() {
            self.anomalies.push("duplicate-supersede".to_string());
            return;
        }
        self.phase = 2;
        self.superseded_by = Some(event.new_claim_id.clone());
        self.superseded_reason.clone_from(&event.reason);
        self.superseded_decided_by.clone_from(&event.decided_by);
    }
}
