//! conflicts command.

use anyhow::Result;
use texo_core::{commit_conflicts, detect_conflicts, open_journal};

use crate::observed_at_ms;

pub fn run(root: &std::path::Path, json: bool, commit: bool) -> Result<()> {
    let journal = open_journal(root)?;
    let workspace = journal.config().workspace()?;
    let replayed = journal.replay(&workspace)?;

    if commit {
        let committed = commit_conflicts(
            journal.handle(),
            &replayed.state,
            workspace.as_str(),
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

    let report = detect_conflicts(&replayed.state, workspace.as_str());
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
