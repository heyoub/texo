//! claims command.

use std::path::Path;

use anyhow::Result;
use texo_core::{build_agent_context, open_journal_with, ClaimStatus};

pub fn run(root: &Path, workspace: Option<&str>, subject: Option<&str>, json: bool) -> Result<()> {
    let journal = open_journal_with(root, workspace)?;
    let workspace_id = journal.config().workspace()?;
    let replayed = journal.replay(&workspace_id)?;
    journal.close()?;

    if json {
        let context = build_agent_context(&replayed.state, workspace_id.as_str(), subject);
        println!("{}", serde_json::to_string_pretty(&context.claims)?);
    } else {
        for claim in replayed.state.claims.values() {
            if subject.is_some_and(|s| claim.subject_hint != s) {
                continue;
            }
            println!(
                "{} {:?} {}",
                claim.claim_id, claim.status, claim.subject_hint
            );
            println!("  \"{}\"", claim.text);
            println!("  source: {}:{}", claim.source_path, claim.line_start);
            println!("  seq: {}", claim.receipt.sequence.get());
            println!("  receipt: {}", claim.receipt.event_id);
        }
        let _ = ClaimStatus::Current;
    }
    Ok(())
}
