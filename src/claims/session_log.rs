//! Session turn projection.

use batpak::event::RawMsgpackInput;
use serde::{Deserialize, Serialize};

use crate::events::payloads::SessionTurnV1;

/// One projected session turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnEntry {
    /// Stable session identifier.
    pub session_id: String,
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// Speaker label.
    pub speaker: String,
    /// Turn text.
    pub text: String,
    /// Monotonic turn number within the session.
    pub turn_no: u32,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
}

/// Session transcript projection.
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventSourced)]
#[batpak(input = RawMsgpackInput, cache_version = 1, state_max_cardinality = 1)]
#[batpak(event = SessionTurnV1, handler = on_turn)]
pub struct SessionLog {
    /// Turns sorted by turn number.
    pub turns: Vec<TurnEntry>,
}

impl SessionLog {
    fn on_turn(&mut self, event: &SessionTurnV1) {
        self.turns.push(TurnEntry {
            session_id: event.session_id.clone(),
            workspace_id: event.workspace_id.clone(),
            speaker: event.speaker.clone(),
            text: event.text.clone(),
            turn_no: event.turn_no,
            observed_at_ms: event.observed_at_ms,
        });
        self.turns.sort_by_key(|turn| turn.turn_no);
    }
}
