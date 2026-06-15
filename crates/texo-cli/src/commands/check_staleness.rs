//! check-staleness command.

use std::path::Path;

use anyhow::Result;
use texo_core::{check_staleness, open_journal};

pub fn run(root: &Path, path: &Path, json: bool) -> Result<()> {
    let journal = open_journal(root)?;
    let workspace = journal.config().workspace()?;
    let replayed = journal.replay(&workspace)?;
    let report = check_staleness(&replayed.state, workspace.as_str(), path, root)?;
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
