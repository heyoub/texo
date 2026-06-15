//! init command.

use std::path::Path;

use anyhow::Result;
use texo_core::init_workspace;

pub fn run(root: &Path, workspace: &str) -> Result<()> {
    let config = init_workspace(root, workspace)?;
    println!(
        "Initialized texo workspace '{}' at {}/.texo",
        config.workspace_id,
        root.display()
    );
    Ok(())
}
