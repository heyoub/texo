//! User-facing surface scaffolding.

/// Command-line interface.
pub mod cli;
/// First-run workspace bootstrap.
pub mod bootstrap;
/// Minimal HTTP client surface.
pub mod http;
/// OpenAI-compatible JSON edge.
#[cfg(feature = "openrouter")]
pub mod openai;
