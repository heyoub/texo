//! conflicts command.

use anyhow::Result;
use texo_core::{commit_conflicts, detect_conflicts, open_journal_with};

use crate::observed_at_ms;

pub fn run(
    root: &std::path::Path,
    workspace: Option<&str>,
    json: bool,
    commit: bool,
) -> Result<()> {
    let journal = open_journal_with(root, workspace)?;
    let workspace_id = journal.config().workspace()?;
    let replayed = journal.replay(&workspace_id)?;

    if commit {
        let committed = commit_conflicts(
            journal.handle(),
            &replayed.state,
            workspace_id.as_str(),
            observed_at_ms(),
        )?;
        journal.close()?;
        if json {
            println!("{}", serde_json::to_string_pretty(&committed)?);
        } else {
            println!("committed {} conflicts", committed.len());
        }
        return Ok(());
    }

    let report = detect_conflicts(&replayed.state, workspace_id.as_str());
    journal.close()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for entry in &report.conflicts {
            println!(
                "{} {} vs {} ({})",
                entry.conflict_id, entry.claim_a, entry.claim_b, entry.subject_hint
            );
        }
    }
    Ok(())
}
