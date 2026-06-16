//! Top-level error type for texo-core.

use crate::config::ConfigError;
use crate::extract::ExtractError;
use crate::journal::JournalError;
use crate::replay::ReplayError;
use crate::source::SourceError;
use crate::state::TransitionError;
use crate::types::IdParseError;

/// Unified error surface for the texo core library.
#[derive(Debug, thiserror::Error)]
pub enum TexoError {
    /// Journal / BatPak store errors.
    #[error("journal: {0}")]
    Journal(#[from] JournalError),
    /// Replay and projection errors.
    #[error("replay: {0}")]
    Replay(#[from] ReplayError),
    /// Claim lifecycle transition errors.
    #[error("transition: {0}")]
    Transition(#[from] TransitionError),
    /// Markdown extraction errors.
    #[error("extract: {0}")]
    Extract(#[from] ExtractError),
    /// Source parsing errors.
    #[error("source: {0}")]
    Source(#[from] SourceError),
    /// Configuration errors.
    #[error("config: {0}")]
    Config(#[from] ConfigError),
    /// I/O errors.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// JSON serialization errors.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// Identifier parse errors.
    #[error("{0}")]
    IdParse(#[from] IdParseError),
    /// Generic domain error with context.
    #[error("{0}")]
    Domain(String),
}

impl TexoError {
    /// Wrap a static message into a domain error.
    pub fn domain(message: impl Into<String>) -> Self {
        Self::Domain(message.into())
    }
}
