//! Conflict courtroom integration test.

mod support;

use serde_json::json;
use support::{TestResult, TestWorkspace, OBSERVED_AT_MS};

#[test]
fn conflict_commit_lists_open_conflict() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    workspace.write("docs/monday.md", "Releases happen on Monday.\n")?;
    workspace.write("docs/friday.md", "Releases go out on Friday.\n")?;
    let _first = workspace.invoke(
        "texo.ingest.run",
        json!({"path": "docs/monday.md", "dry_run": false, "observed_at_ms": OBSERVED_AT_MS + 1}),
    )?;
    let _second = workspace.invoke(
        "texo.ingest.run",
        json!({"path": "docs/friday.md", "dry_run": false, "observed_at_ms": OBSERVED_AT_MS + 2}),
    )?;
    let committed = workspace.invoke(
        "texo.conflicts.commit",
        json!({"observed_at_ms": OBSERVED_AT_MS + 3}),
    )?;
    assert_eq!(committed.as_array().map(Vec::len), Some(1));
    let listed = workspace.invoke("texo.conflicts.list", json!({}))?;
    assert_eq!(listed["open"].as_array().map(Vec::len), Some(1));
    Ok(())
}
