//! Idempotent ingest/replay integration test.

mod support;

use serde_json::json;
use support::{TestResult, TestWorkspace, OBSERVED_AT_MS};

#[test]
fn unchanged_source_is_not_reingested() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    workspace.write("docs/policy.md", "Deploys happen on Friday.\n")?;
    let first = workspace.invoke(
        "texo.ingest.run",
        json!({"path": "docs/policy.md", "dry_run": false, "observed_at_ms": OBSERVED_AT_MS + 1}),
    )?;
    let second = workspace.invoke(
        "texo.ingest.run",
        json!({"path": "docs/policy.md", "dry_run": false, "observed_at_ms": OBSERVED_AT_MS + 2}),
    )?;
    assert_eq!(first["claims_recorded"], 1);
    assert_eq!(second["claims_recorded"], 0);
    Ok(())
}
