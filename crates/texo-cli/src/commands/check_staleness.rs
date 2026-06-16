//! check-staleness command.

use std::path::Path;

use anyhow::Result;
use texo_core::{check_staleness, open_journal_with};

pub fn run(root: &Path, workspace: Option<&str>, path: &Path, json: bool) -> Result<()> {
    let journal = open_journal_with(root, workspace)?;
    let workspace_id = journal.config().workspace()?;
    let replayed = journal.replay(&workspace_id)?;
    let report = check_staleness(&replayed.state, &workspace_id, path, root)?;
    journal.close()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for diag in &report.diagnostics {
            println!(
                "{}:{} warning — {}",
                diag.file, diag.line_start, diag.message
            );
        }
        if report.diagnostics.is_empty() {
            println!("no stale claims detected");
        }
    }
    Ok(())
}
