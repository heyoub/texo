//! User-facing surface scaffolding.

/// Command-line interface.
pub mod cli;
/// First-run workspace bootstrap.
pub mod bootstrap;
/// Minimal HTTP client surface.
pub mod http;
/// Sync MCP stdio surface.
pub mod mcp_stdio;
/// OpenAI-compatible JSON edge.
#[cfg(feature = "openrouter")]
pub mod openai;
