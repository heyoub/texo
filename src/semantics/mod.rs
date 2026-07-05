//! Semantic relation scaffolding.

/// Chat-completions helpers.
#[cfg(feature = "openrouter")]
pub mod chat;
/// Hosted OpenRouter semantic backends.
#[cfg(feature = "openrouter")]
pub mod openrouter;
pub mod pipeline;
pub mod traits;

pub use traits::*;
