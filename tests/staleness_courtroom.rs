//! Staleness courtroom integration test.

#[path = "support/courtroom.rs"]
mod courtroom_support;
mod support;

use courtroom_support::ingest_courtroom;
use serde_json::json;
use support::{TestResult, TestWorkspace};

#[test]
fn stale_source_line_reports_supersession() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    ingest_courtroom(&mut workspace)?;
    let report = workspace.invoke("texo.staleness.check", &json!({"path": "docs/friday.md"}))?;
    let diagnostics = report["diagnostics"].as_array().expect("diagnostics array");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["line_start"], 1);
    assert!(diagnostics[0]["message"]
        .as_str()
        .is_some_and(|message| message.contains("superseded by")));
    Ok(())
}
