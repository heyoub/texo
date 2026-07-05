//! `texo-agent` — memory agent HTTP server over a texo claim-chain workspace.
//!
//! Environment:
//! - `TEXO_AGENT_ROOT` — workspace root (default `.`); bootstrapped on first
//!   run with `docs_glob = sessions/**/*.md`, `extractor_cmd` = the
//!   `texo-extract` binary, and `[semantics] enabled = true`.
//! - `TEXO_AGENT_ADDR` — listen address (default `127.0.0.1:8787`).
//! - `TEXO_AGENT_WORKSPACE` — workspace scope id (default `memory`).
//! - `TEXO_EXTRACT_BIN` — path to `texo-extract` (default: sibling of this
//!   executable).
//! - `OPENROUTER_BASE_URL` / `OPENROUTER_API_KEY` / `OPENROUTER_CHAT_MODEL` —
//!   OpenAI-compatible chat backend (texo-semantics conventions).
//! - `TEXO_EXTRACT_CACHE` / `TEXO_RELATE_CACHE` — record-once caches (default:
//!   under `<root>/.texo/`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use texo_agent::bootstrap::{
    ensure_workspace, extractor_cmd_for, resolve_extract_bin, BootstrapOptions,
};
use texo_agent::chat::ChatConfig;
use texo_agent::server::{serve, AppState};
use texo_agent::session::SessionStore;

/// Default listen address.
const DEFAULT_ADDR: &str = "127.0.0.1:8787";
/// Default workspace scope id.
const DEFAULT_WORKSPACE: &str = "memory";
/// Per-request timeout for the chat model (generous: reasoning models are
/// slow; matches the texo-semantics request timeout).
const CHAT_TIMEOUT: Duration = Duration::from_secs(120);

fn env_nonblank(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

#[tokio::main]
async fn main() -> Result<()> {
    let root = PathBuf::from(env_nonblank("TEXO_AGENT_ROOT").unwrap_or_else(|| ".".to_owned()));
    // Canonicalize so the extractor-cache path baked into the workspace config
    // is absolute (the extractor subprocess runs with its cwd at sessions/).
    let root = root
        .canonicalize()
        .with_context(|| format!("workspace root {} does not exist", root.display()))?;
    let addr = env_nonblank("TEXO_AGENT_ADDR").unwrap_or_else(|| DEFAULT_ADDR.to_owned());
    let workspace = env_nonblank("TEXO_AGENT_WORKSPACE");

    let extractor_cmd = resolve_extract_bin().map(|bin| extractor_cmd_for(&root, &bin));
    if extractor_cmd.is_none() {
        eprintln!(
            "warning: texo-extract binary not found (set TEXO_EXTRACT_BIN or build it \
             next to texo-agent); bootstrapping with the heuristic extractor"
        );
    }
    let semantics_enabled = extractor_cmd.is_some();
    ensure_workspace(
        &root,
        &BootstrapOptions {
            workspace_id: workspace
                .clone()
                .unwrap_or_else(|| DEFAULT_WORKSPACE.to_owned()),
            extractor_cmd,
            semantics_enabled,
        },
    )
    .context("bootstrapping texo workspace")?;

    let chat = ChatConfig::from_env();
    match &chat {
        Some(config) => println!("chat model: {} via {}", config.model, config.base_url),
        None => eprintln!(
            "warning: OPENROUTER_API_KEY not set — /api/chat disabled; memory endpoints \
             still work"
        ),
    }

    let http = reqwest::Client::builder()
        .timeout(CHAT_TIMEOUT)
        .build()
        .context("building HTTP client")?;

    println!("workspace root: {}", root.display());
    let state = Arc::new(AppState {
        root,
        workspace,
        sessions: SessionStore::new(),
        chat,
        http,
    });
    serve(state, &addr).await
}
