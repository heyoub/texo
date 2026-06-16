//! relate command — the semantic supersession + conflict pass.
//!
//! Reads the current claims from the journal, asks the OpenRouter-backed
//! [`relate_claims`] pipeline how they relate (meaning-based supersession +
//! mutual-contradiction conflict), and journals the resulting `ClaimSuperseded`
//! and `ClaimConflictDetected` events. This is the record-once boundary for
//! relations: the model runs here, the *events* are the recorded output, and
//! replay/compile stay deterministic over them.
//!
//! Requires `OPENROUTER_API_KEY`. Lives in the CLI (not `texo-core`) because it
//! injects the hosted backends, keeping `texo-core` HTTP/LLM-free.

use std::path::Path;

use anyhow::{Context, Result};
use texo_core::{
    open_journal_with, relate_claims, ClaimConflictDetected, ClaimId, ClaimStatus, ClaimSuperseded,
    ClaimView,
};
use texo_semantics::{OpenRouterEmbedder, OpenRouterRelater};

use crate::observed_at_ms;

/// Coarse cosine prefilter for relating: must sit below the lowest same-subject
/// similarity (the relater does the real separation), so it is intentionally
/// lower than the grouping `cosine_threshold` in `[semantics]`.
const PREFILTER: f32 = 0.60;

pub fn run(root: &Path, workspace: Option<&str>, json: bool) -> Result<()> {
    let journal = open_journal_with(root, workspace)?;
    let workspace_id = journal.config().workspace()?;
    let replayed = journal.replay(&workspace_id)?;

    // Current claims only, ordered by journal sequence (then id) for stable runs.
    let mut claims: Vec<(ClaimId, ClaimView)> = replayed
        .state
        .claims
        .iter()
        .filter(|(_, view)| view.status == ClaimStatus::Current)
        .map(|(id, view)| (id.clone(), view.clone()))
        .collect();
    claims.sort_by(|a, b| {
        a.1.receipt
            .sequence
            .get()
            .cmp(&b.1.receipt.sequence.get())
            .then_with(|| a.0.as_str().cmp(b.0.as_str()))
    });

    let embedder = OpenRouterEmbedder::new(None).context("building OpenRouter embedder")?;
    let relater = OpenRouterRelater::new(None).context("building OpenRouter relater")?;
    let out = relate_claims(&claims, &embedder, &relater, PREFILTER).context("relating claims")?;

    let now = observed_at_ms();
    let handle = journal.handle();
    for (old, new, reason) in &out.supersessions {
        handle.append_superseded(&ClaimSuperseded {
            old_claim_id: old.to_string(),
            new_claim_id: new.to_string(),
            workspace_id: workspace_id.to_string(),
            reason: reason.clone(),
            decided_by: "texo-relate".to_string(),
            observed_at_ms: now,
        })?;
    }
    for entry in &out.conflicts {
        handle.append_conflict(&ClaimConflictDetected {
            conflict_id: entry.conflict_id.to_string(),
            workspace_id: workspace_id.to_string(),
            claim_a: entry.claim_a.to_string(),
            claim_b: entry.claim_b.to_string(),
            reason: entry.reason.clone(),
            status: "open".to_string(),
            observed_at_ms: now,
        })?;
    }

    let superseded = out.supersessions.len();
    let conflicts = out.conflicts.len();
    journal.close()?;

    if json {
        println!(
            "{{\"claims_related\":{},\"supersessions\":{superseded},\"conflicts\":{conflicts}}}",
            claims.len()
        );
    } else {
        println!(
            "related {} claims: {superseded} supersessions, {conflicts} conflicts ({workspace_id})",
            claims.len()
        );
    }
    Ok(())
}
