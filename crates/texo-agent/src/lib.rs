//! texo-agent — an HTTP chat agent whose persistent memory is a texo
//! claim-chain workspace.
//!
//! Every memory agent accumulates; this one *retires*. Facts the user states
//! across sessions become claims in a BatPak-backed texo journal. When a fact
//! changes, texo's supersession machinery retires the old claim with a receipt
//! — what superseded it, when, from which source line — instead of letting
//! contradictory memories pile up and hoping retrieval ranks the right one.
//!
//! The loop:
//!
//! 1. **Chat** (`POST /api/chat`) — the system prompt injects the *current*
//!    claims replayed from the journal as trusted memory (with `path:line`
//!    provenance) and lists recently superseded claims as outdated.
//! 2. **Session end** (`POST /api/session/end`) — the transcript is rendered
//!    to `sessions/<session_id>.md`, ingested through the workspace's
//!    configured extractor (the `texo-extract` LLM path in production, the
//!    heuristic in tests), then the semantic relate pass supersedes/conflicts
//!    claims. The *next* session sees the updated memory.
//! 3. **Sidebar** (`GET /api/memory`) — current, stale, and conflicting claims
//!    replayed from the journal, powering the live UI at `/`.
//!
//! Boundaries: BatPak I/O goes through texo-core journal APIs only, always on
//! `spawn_blocking` worker threads (the texo-mcp pattern). Session transcripts
//! live in process memory; the journal is the durable state.

#![warn(missing_docs)]

pub mod bootstrap;
pub mod chat;
pub mod memory;
pub mod server;
pub mod session;
