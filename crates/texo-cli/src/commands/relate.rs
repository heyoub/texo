//! relate command — the semantic supersession + conflict pass.
//!
//! Reads the current claims from the journal, asks the OpenRouter-backed
//! [`relate_claims`] pipeline how they relate (meaning-based supersession +
//! mutual-contradiction conflict), and journals the resulting `ClaimSuperseded`
//! and `ClaimConflictDetected` events. This is the record-once boundary for
//! relations: the model runs here, the *events* are the recorded output, and
//! replay/compile stay deterministic over them.
//!
//! Candidate generation is cluster-first: claims are clustered by connected
//! components over the cosine-similarity graph (at the `[semantics]`
//! `cosine_threshold`) and the LLM judge only sees *within-cluster* pairs, which
//! bounds judge calls by cluster size instead of O(n²) over the corpus. Pairs
//! split across clusters are deliberately never judged (see [`relate_claims`]).
//!
//! Requires `OPENROUTER_API_KEY`. Lives in the CLI (not `texo-core`) because it
//! injects the hosted backends, keeping `texo-core` HTTP/LLM-free.

use std::path::Path;

use anyhow::{Context, Result};
use texo_core::{
    open_journal_with, relate_claims, ClaimConflictDetected, ClaimId, ClaimStatus, ClaimSuperseded,
    ClaimView, RelateThresholds, SemanticsConfig,
};
use texo_extract::CachingRelater;
use texo_semantics::{OpenRouterEmbedder, OpenRouterRelater};

use crate::observed_at_ms;

/// Coarse cosine prefilter for relating: must sit below the lowest same-subject
/// similarity (the relater does the real separation), so it is intentionally
/// lower than the clustering `cosine_threshold` in `[semantics]`.
const PREFILTER: f32 = 0.60;

/// Environment variable selecting the record-once relate cache directory.
const ENV_RELATE_CACHE: &str = "TEXO_RELATE_CACHE";
/// Default relate cache directory, relative to the workspace root.
const DEFAULT_RELATE_CACHE: &str = ".texo/relate-cache";

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
    // Cache the relater (the pairwise judging step) so a re-run after a transient
    // failure resumes from already-judged pairs instead of repeating the pass.
    let cache_dir = std::env::var_os(ENV_RELATE_CACHE)
        .map_or_else(|| root.join(DEFAULT_RELATE_CACHE), std::path::PathBuf::from);
    let relater = CachingRelater::new(
        OpenRouterRelater::new(None).context("building OpenRouter relater")?,
        cache_dir,
    );
    // Cluster link threshold: the workspace `[semantics]` cosine_threshold, or
    // its default when the workspace has no semantics table.
    let cluster = journal.config().semantics.as_ref().map_or_else(
        || SemanticsConfig::default().cosine_threshold,
        |semantics| semantics.cosine_threshold,
    );
    let thresholds = RelateThresholds {
        cluster,
        prefilter: PREFILTER,
    };
    let out = relate_claims(&claims, &embedder, &relater, thresholds).context("relating claims")?;

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

    if json {
        println!(
            "{{\"claims_related\":{},\"supersessions\":{superseded},\"conflicts\":{conflicts}}}",
            claims.len()
        );
    } else {
        let text_of = |id: &ClaimId| -> String {
            claims
                .iter()
                .find(|(cid, _)| cid == id)
                .map_or_else(|| id.to_string(), |(_, v)| v.text.clone())
        };
        println!(
            "related {} claims: {superseded} supersessions, {conflicts} conflicts ({workspace_id})",
            claims.len()
        );
        for (old, new, _) in &out.supersessions {
            println!("  superseded: {:?}  ->  {:?}", text_of(old), text_of(new));
        }
        for entry in &out.conflicts {
            println!(
                "  conflict:   {:?}  <>  {:?}",
                text_of(&entry.claim_a),
                text_of(&entry.claim_b)
            );
        }
    }
    journal.close()?;
    Ok(())
}
