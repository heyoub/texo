//! ingest command.

use std::path::Path;

use anyhow::Result;
use texo_core::{ingest_sources, open_journal_with, IngestMode, IngestReport};

use crate::observed_at_ms;

pub fn run(
    root: &Path,
    path: &Path,
    workspace: Option<&str>,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    let journal = open_journal_with(root, workspace)?;
    let config = journal.config().clone();
    let workspace_id = config.workspace()?;
    let mode = if dry_run {
        IngestMode::DryRun
    } else {
        IngestMode::Commit
    };
    let committed = ingest_sources(
        journal.handle(),
        &config,
        &workspace_id,
        path,
        mode,
        observed_at_ms(),
        root,
    )?;
    journal.close()?;

    if json {
        let report = IngestReport::from(committed);
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "ingested {} sources, {} claims ({})",
            committed.sources_observed, committed.claims_recorded, committed.workspace_id
        );
    }
    Ok(())
}
