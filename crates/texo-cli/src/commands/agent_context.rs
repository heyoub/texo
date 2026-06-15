//! agent-context command.

use std::path::Path;

use anyhow::Result;
use texo_core::{build_agent_context, open_journal};

pub fn run(root: &Path, subject: Option<&str>, out: Option<&Path>, json: bool) -> Result<()> {
    let journal = open_journal(root)?;
    let workspace = journal.config().workspace()?;
    let replayed = journal.replay(&workspace)?;
    let context = build_agent_context(&replayed.state, workspace.as_str(), subject);
    journal.close()?;

    let rendered = serde_json::to_string_pretty(&context)?;
    if let Some(path) = out {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, rendered)?;
    } else if json || out.is_none() {
        println!("{rendered}");
    }
    Ok(())
}
