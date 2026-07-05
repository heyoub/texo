//! Conflict card projection.

use batpak::event::RawMsgpackInput;
use serde::{Deserialize, Serialize};

use crate::events::payloads::{ConflictOpenedV2, ConflictResolvedV2};

/// Current card view for a conflict entity.
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventSourced)]
#[batpak(input = RawMsgpackInput, cache_version = 1, state_max_cardinality = 1)]
#[batpak(event = ConflictOpenedV2, handler = on_opened)]
#[batpak(event = ConflictResolvedV2, handler = on_resolved)]
pub struct ConflictCard {
    /// Conflict phase: 0 unopened, 1 open, 2 resolved, 3 ignored.
    pub phase: u64,
    /// Stable conflict identifier.
    pub conflict_id: String,
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// First conflicting claim identifier.
    pub claim_a: String,
    /// Second conflicting claim identifier.
    pub claim_b: String,
    /// Human-readable conflict reason.
    pub reason: String,
    /// Detector implementation that opened the conflict.
    pub detector: String,
    /// Resolution value.
    pub resolution: String,
    /// Actor that resolved or ignored the conflict.
    pub resolved_by: String,
    /// Open wall-clock time in milliseconds.
    pub opened_at_ms: u64,
    /// Resolution wall-clock time in milliseconds.
    pub resolved_at_ms: u64,
    /// Domain lifecycle anomalies observed during replay.
    pub anomalies: Vec<String>,
}

impl ConflictCard {
    fn on_opened(&mut self, event: &ConflictOpenedV2) {
        if self.phase != 0 {
            self.anomalies.push("duplicate-open".to_string());
        }
        self.phase = 1;
        self.conflict_id.clone_from(&event.conflict_id);
        self.workspace_id.clone_from(&event.workspace_id);
        self.claim_a.clone_from(&event.claim_a);
        self.claim_b.clone_from(&event.claim_b);
        self.reason.clone_from(&event.reason);
        self.detector.clone_from(&event.detector);
        self.opened_at_ms = event.observed_at_ms;
    }

    fn on_resolved(&mut self, event: &ConflictResolvedV2) {
        if self.phase != 1 {
            self.anomalies.push(format!("resolve-from-{}", self.phase));
        }
        self.phase = match event.resolution.as_str() {
            "resolved" => 2,
            "ignored" => 3,
            _ => {
                self.anomalies.push("unknown-resolution".to_string());
                self.phase
            }
        };
        self.conflict_id.clone_from(&event.conflict_id);
        self.workspace_id.clone_from(&event.workspace_id);
        self.resolution.clone_from(&event.resolution);
        self.resolved_by.clone_from(&event.resolved_by);
        self.resolved_at_ms = event.observed_at_ms;
    }
}
