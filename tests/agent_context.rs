//! Agent context integration test.

mod support;

use serde_json::json;
use support::{ingest_courtroom, TestResult, TestWorkspace};

#[test]
fn agent_context_contains_current_and_stale_claims() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    ingest_courtroom(&mut workspace)?;
    let context = workspace.invoke(
        "texo.context.agent",
        json!({"subject": null, "include_stale": true}),
    )?;
    assert_eq!(context["workspace_id"], "demo");
    assert!(context["claims"]
        .as_array()
        .is_some_and(|claims| claims.len() == 1));
    assert!(context["stale_claims"]
        .as_array()
        .is_some_and(|claims| claims.len() == 1));
    Ok(())
}
