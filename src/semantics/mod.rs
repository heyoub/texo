//! Semantic relation scaffolding.

/// Chat-completions helpers.
#[cfg(feature = "openrouter")]
pub mod chat;
/// Hosted OpenRouter semantic backends.
#[cfg(feature = "openrouter")]
pub mod openrouter;
/// Semantic claim relation pipeline.
pub mod pipeline;
pub(crate) mod score;
pub mod traits;

pub use traits::*;
