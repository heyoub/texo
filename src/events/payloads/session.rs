use serde::{Deserialize, Serialize};

/// One turn in a session transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 8, version = 1)]
pub struct SessionTurnV1 {
    /// Stable session identifier.
    pub session_id: String,
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// Speaker label, either `user` or `assistant`.
    pub speaker: String,
    /// Turn text.
    pub text: String,
    /// Monotonic turn number within the session.
    pub turn_no: u32,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
}
