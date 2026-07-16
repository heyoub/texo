//! Deploy-change fixture used only by integration tests that request it.

use serde_json::json;

use crate::support::{TestResult, TestWorkspace, OBSERVED_AT_MS};

/// Populate and ingest the deterministic deploy-change fixture.
pub fn ingest_courtroom(workspace: &mut TestWorkspace) -> TestResult {
    workspace.write("docs/friday.md", "Deploys happen on Friday.\n")?;
    workspace.write("docs/tuesday.md", "Decision: deploys moved to Tuesday.\n")?;
    let _first = workspace.invoke(
        "texo.ingest.run",
        &json!({"path": "docs/friday.md", "dry_run": false, "observed_at_ms": OBSERVED_AT_MS + 1}),
    )?;
    let _second = workspace.invoke(
        "texo.ingest.run",
        &json!({"path": "docs/tuesday.md", "dry_run": false, "observed_at_ms": OBSERVED_AT_MS + 2}),
    )?;
    Ok(())
}
