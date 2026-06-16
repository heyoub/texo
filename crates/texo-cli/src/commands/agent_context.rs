//! agent-context command.

use std::path::Path;

use anyhow::Result;
use texo_core::{build_agent_context, open_journal_with};

pub fn run(
    root: &Path,
    workspace: Option<&str>,
    subject: Option<&str>,
    out: Option<&Path>,
    json: bool,
) -> Result<()> {
    let journal = open_journal_with(root, workspace)?;
    let workspace_id = journal.config().workspace()?;
    let replayed = journal.replay(&workspace_id)?;
    let context = build_agent_context(&replayed.state, &workspace_id, subject);
    journal.close()?;

    let rendered = serde_json::to_string_pretty(&context)?;

    // `--out` writes the JSON to a file; `--json` prints JSON to stdout. The two
    // are independent and may both apply. When neither is given there is no
    // human-readable alternative for agent context, so default to stdout JSON.
    if let Some(path) = out {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &rendered)?;
    }
    if json || out.is_none() {
        println!("{rendered}");
    }
    Ok(())
}
