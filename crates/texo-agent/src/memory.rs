//! Memory projections replayed from the claim-chain journal.
//!
//! Everything here is synchronous and performs BatPak I/O through texo-core's
//! journal APIs; the HTTP layer calls it on `spawn_blocking` worker threads
//! (the texo-mcp pattern). The projection reuses [`build_agent_context`] — the
//! same replay the CLI's `claims`/`agent-context` commands surface — and joins
//! in the span-level byte offsets carried by the `ClaimRecorded` events.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use texo_core::{build_agent_context, open_journal_with, TexoEvent};

/// One current claim surfaced as trusted memory, with its receipt: source
/// path, line, and the span's byte range in that source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryClaim {
    /// Claim id.
    pub claim_id: String,
    /// Claim text.
    pub text: String,
    /// Source document path (relative to the docs root).
    pub source_path: String,
    /// 1-based line the claim's source span starts on.
    pub line: u32,
    /// Byte offset (inclusive) of the claim's source span start.
    pub char_start: u32,
    /// Byte offset (exclusive) of the claim's source span end.
    pub char_end: u32,
}

/// A retired memory: superseded with a receipt, kept for provenance, never
/// trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StaleMemory {
    /// Superseded claim id.
    pub claim_id: String,
    /// The outdated text.
    pub text: String,
    /// Id of the claim that superseded it.
    pub superseded_by: String,
    /// Text of the superseding claim (empty if it left the projection).
    pub superseded_by_text: String,
}

/// An unresolved contradiction between two co-current memories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryConflict {
    /// First claim's text.
    pub claim_a_text: String,
    /// Second claim's text.
    pub claim_b_text: String,
    /// Why the pair conflicts.
    pub reason: String,
}

/// Full memory snapshot replayed from the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemorySnapshot {
    /// Workspace id the snapshot was replayed for.
    pub workspace_id: String,
    /// Replay frontier (local store sequence).
    pub replayed_through_sequence: u64,
    /// Current claims — the trusted memory.
    pub current: Vec<MemoryClaim>,
    /// Superseded claims — outdated, with what replaced them.
    pub stale: Vec<StaleMemory>,
    /// Open conflicts — both claimed, neither wins.
    pub conflicts: Vec<MemoryConflict>,
}

/// Replay the journal into a [`MemorySnapshot`].
///
/// Ordering is deterministic: current claims by journal sequence, stale claims
/// by claim id, conflicts deduplicated by text pair (all inherited from
/// [`build_agent_context`]).
pub fn load_memory(root: &Path, workspace: Option<&str>) -> Result<MemorySnapshot> {
    let journal = open_journal_with(root, workspace).context("opening texo journal")?;
    let workspace_id = journal.config().workspace()?;
    let replayed = journal.replay(&workspace_id)?;
    // Span byte offsets live on the ClaimRecorded payloads; the replayed
    // ClaimView does not carry them, so one pass over the raw events recovers
    // claim id -> (char_start, char_end).
    let events = journal.handle().load_events(&workspace_id)?;
    journal.close()?;

    let mut spans: HashMap<&str, (u32, u32)> = HashMap::new();
    for event in &events {
        if let TexoEvent::ClaimRecorded { payload, .. } = event {
            spans.insert(&payload.claim_id, (payload.char_start, payload.char_end));
        }
    }

    let context = build_agent_context(&replayed.state, &workspace_id, None);

    let current = context
        .claims
        .iter()
        .map(|claim| {
            let (char_start, char_end) = spans
                .get(claim.claim_id.as_str())
                .copied()
                .unwrap_or((0, 0));
            MemoryClaim {
                claim_id: claim.claim_id.to_string(),
                text: claim.text.clone(),
                source_path: claim.source.path.clone(),
                line: claim.source.line_start,
                char_start,
                char_end,
            }
        })
        .collect();

    let stale = context
        .stale_claims
        .iter()
        .map(|stale| StaleMemory {
            claim_id: stale.claim_id.to_string(),
            text: stale.text.clone(),
            superseded_by: stale.superseded_by.to_string(),
            superseded_by_text: replayed
                .state
                .claim(&stale.superseded_by)
                .map(|c| c.text.clone())
                .unwrap_or_default(),
        })
        .collect();

    let conflicts = context
        .conflicts
        .iter()
        .map(|conflict| MemoryConflict {
            claim_a_text: conflict.claim_a_text.clone(),
            claim_b_text: conflict.claim_b_text.clone(),
            reason: conflict.reason.clone(),
        })
        .collect();

    Ok(MemorySnapshot {
        workspace_id: workspace_id.to_string(),
        replayed_through_sequence: context.replayed_through_sequence,
        current,
        stale,
        conflicts,
    })
}
