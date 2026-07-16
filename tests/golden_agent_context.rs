//! Golden agent-context snapshot.

#[path = "support/sample.rs"]
mod sample_support;
mod support;

use sample_support::ingest_sample_sources;
use serde_json::json;
use support::{TestResult, TestWorkspace};

#[test]
fn agent_context_demo() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    let _report = ingest_sample_sources(&mut workspace)?;
    let context = workspace.invoke(
        "texo.context.agent",
        &json!({"subject": null, "include_stale": true, "allow_unsettled": true}),
    )?;
    insta::assert_json_snapshot!("agent_context_demo", context, {
        ".claims[].receipt.event_id" => "[event-id]",
        ".claims[].receipt.sequence" => "[sequence]",
        ".replayed_through_sequence" => "[frontier]",
        ".freshness.description" => "[freshness]"
    });
    Ok(())
}
