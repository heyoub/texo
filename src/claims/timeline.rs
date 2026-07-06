//! Claim timeline projection.

use batpak::event::RawMsgpackInput;
use serde::{Deserialize, Serialize};

use crate::events::payloads::{ClaimRecordedV2, ClaimSupersededV2};

/// One claim timeline entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineEntry {
    /// Timeline event kind.
    pub kind: String,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
    /// Human-readable timeline summary.
    pub summary: String,
}

/// Arrival-ordered timeline for a claim entity.
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventSourced)]
#[batpak(input = RawMsgpackInput, cache_version = 1, state_max_cardinality = 1)]
#[batpak(event = ClaimRecordedV2, handler = on_recorded)]
#[batpak(event = ClaimSupersededV2, handler = on_superseded)]
pub struct ClaimTimeline {
    /// Timeline entries in replay arrival order.
    pub entries: Vec<TimelineEntry>,
}

impl ClaimTimeline {
    fn on_recorded(&mut self, event: &ClaimRecordedV2) {
        self.entries.push(TimelineEntry {
            kind: "recorded".to_string(),
            observed_at_ms: event.observed_at_ms,
            summary: event.claim_id.clone(),
        });
    }

    fn on_superseded(&mut self, event: &ClaimSupersededV2) {
        self.entries.push(TimelineEntry {
            kind: "superseded".to_string(),
            observed_at_ms: event.observed_at_ms,
            summary: format!("{} -> {}", event.old_claim_id, event.new_claim_id),
        });
    }
}
