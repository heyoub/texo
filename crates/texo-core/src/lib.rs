//! texo-core — context version control for claims.

#![warn(missing_docs)]

pub mod agent;
pub mod config;
pub mod conflicts;
pub mod error;
pub mod events;
pub mod extract;
pub mod fixture;
pub mod ingest;
pub mod journal;
pub mod render;
pub mod replay;
pub mod source;
pub mod stale;
pub mod state;
pub mod types;

pub use agent::{
    build_agent_context, explain_claim, AgentClaim, AgentContext, AgentStaleClaim,
    ClaimExplanation, FreshnessView,
};
pub use config::{ConfigError, TexoConfig, TexoRootConfig, WorkspaceConfig, WorkspaceEntry};
pub use conflicts::{
    commit_conflicts, detect_conflicts, verify_journal_receipts, verify_projection, verify_store,
    VerifyError,
};
pub use error::TexoError;
pub use events::{
    ClaimConflictDetected, ClaimRecorded, ClaimSuperseded, OnboardingCompiled, SourceObserved,
    TexoEvent,
};
pub use extract::{
    extract_claims, extract_via_cmd, ExtractClaimsFn, ExtractError, ExtractedClaim,
    EXTRACTOR_KIND_HEURISTIC_V1,
};
pub use fixture::{
    DEFAULT_CONFIG_DIR, DEFAULT_STORE_PATH, DEFAULT_WORKSPACE_ID, FIXTURE_OBSERVED_AT_MS,
};
pub use journal::{ingest_sources, plan_ingest_sources, JournalError, StoreHandle};
pub use render::{compile_artifacts, render_onboarding, CompileOutput};
pub use replay::{ClaimState, ClaimView, ReplayError, ReplayedState};
pub use source::{collect_markdown_files, MarkdownDocument, SourceError};
pub use stale::{check_staleness, StalenessReport};
pub use state::{
    Closed, IngestCommitted, IngestMode, IngestPlan, IngestReport, Journal, Open, TransitionError,
};
pub use types::{
    ClaimId, ClaimStatus, ConflictId, ConflictStatus, LocalSequence, ObservedAtMs, ReceiptView,
    ReplayFrontier, SourceId, WorkspaceId,
};

use std::path::Path;

/// Initialize `.texo/config.toml` and store directory for one workspace scope.
pub fn init_workspace(root: &Path, workspace_id: &str) -> Result<WorkspaceConfig, TexoError> {
    let config_path = root.join(fixture::DEFAULT_CONFIG_DIR).join("config.toml");
    let mut root_config = if config_path.exists() {
        TexoRootConfig::load(&config_path)?
    } else {
        TexoRootConfig::demo()
    };
    root_config.upsert_workspace(workspace_id, WorkspaceEntry::for_id(workspace_id));
    root_config.save(&config_path)?;
    let config = root_config.resolve(Some(workspace_id))?;
    let store_path = config.store_path_buf(root);
    if let Some(parent) = store_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(config)
}

/// Open an on-disk journal using config from `.texo/config.toml`.
pub fn open_journal(root: &Path) -> Result<Journal<Open>, TexoError> {
    open_journal_with(root, None)
}

/// Open a journal for an explicit workspace id (or default when `None`).
pub fn open_journal_with(
    root: &Path,
    workspace_id: Option<&str>,
) -> Result<Journal<Open>, TexoError> {
    let config_path = root.join(fixture::DEFAULT_CONFIG_DIR).join("config.toml");
    let root_config = TexoRootConfig::load(&config_path)?;
    let config = root_config.resolve(workspace_id)?;
    Journal::open(config, root)
}

/// Compile artifacts into an output directory and journal the compile event.
pub fn compile_out(
    root: &Path,
    out: &Path,
    observed_at_ms: u64,
    workspace_id: Option<&str>,
) -> Result<CompileOutput, TexoError> {
    let journal = open_journal_with(root, workspace_id)?;
    let workspace = journal.config().workspace()?;
    let replayed = journal.replay(&workspace)?;
    let context = build_agent_context(&replayed.state, workspace.as_str(), None);
    let docs_root = journal.config().docs_scan_root(root);
    let stale = check_staleness(&replayed.state, workspace.as_str(), &docs_root, root)?;
    let conflicts = detect_conflicts(&replayed.state, workspace.as_str());
    let output = compile_artifacts(&context, &replayed.state, &stale, &conflicts)?;

    std::fs::create_dir_all(out)?;
    for (name, content) in &output.files {
        std::fs::write(out.join(name), content)?;
    }

    let source_claim_ids: Vec<String> = context
        .claims
        .iter()
        .map(|c| c.claim_id.to_string())
        .collect();
    let payload = OnboardingCompiled {
        doc_id: "doc_onboarding".to_string(),
        workspace_id: workspace.to_string(),
        output_path: out
            .strip_prefix(root)
            .unwrap_or(out)
            .join("onboarding.generated.md")
            .to_string_lossy()
            .to_string(),
        source_claim_ids,
        replayed_through_sequence: replayed.state.replayed_through_sequence,
        compiled_at_ms: observed_at_ms,
    };
    journal.handle().append_onboarding_compiled(&payload)?;
    journal.close()?;
    Ok(output)
}

/// Manual supersession append.
pub fn supersede_claim(
    root: &Path,
    old_claim_id: &ClaimId,
    new_claim_id: &ClaimId,
    reason: &str,
    decided_by: &str,
    observed_at_ms: u64,
    workspace_id: Option<&str>,
) -> Result<ReceiptView, TexoError> {
    let journal = open_journal_with(root, workspace_id)?;
    let workspace = journal.config().workspace()?;
    let payload = ClaimSuperseded {
        old_claim_id: old_claim_id.to_string(),
        new_claim_id: new_claim_id.to_string(),
        workspace_id: workspace.to_string(),
        reason: reason.to_string(),
        decided_by: decided_by.to_string(),
        observed_at_ms,
    };
    let receipt = journal.handle().append_superseded(&payload)?;
    journal.close()?;
    Ok(receipt)
}
