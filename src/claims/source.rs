//! Source card projection.

use batpak::event::RawMsgpackInput;
use serde::{Deserialize, Serialize};

use crate::events::payloads::SourceObservedV2;

/// Latest source observation plus observation count.
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventSourced)]
#[batpak(input = RawMsgpackInput, cache_version = 1, state_max_cardinality = 1)]
#[batpak(event = SourceObservedV2, handler = on_observed)]
pub struct SourceCard {
    /// Stable source identifier.
    pub source_id: String,
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// Source kind label.
    pub source_kind: String,
    /// Source path relative to the workspace root.
    pub path: String,
    /// BLAKE3 body hash as lowercase hex.
    pub body_hash_hex: String,
    /// Latest observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
    /// Number of observations replayed for this source.
    pub observations: u32,
}

impl SourceCard {
    fn on_observed(&mut self, event: &SourceObservedV2) {
        self.source_id.clone_from(&event.source_id);
        self.workspace_id.clone_from(&event.workspace_id);
        self.source_kind.clone_from(&event.source_kind);
        self.path.clone_from(&event.path);
        self.body_hash_hex.clone_from(&event.body_hash_hex);
        self.observed_at_ms = event.observed_at_ms;
        self.observations = self.observations.saturating_add(1);
    }
}
