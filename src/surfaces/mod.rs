//! User-facing surface scaffolding.

/// Command-line interface.
pub mod cli;
/// Minimal HTTP client surface.
#[cfg(feature = "openrouter")]
pub mod http;
/// OpenAI-compatible JSON edge.
#[cfg(feature = "openrouter")]
pub mod openai;
