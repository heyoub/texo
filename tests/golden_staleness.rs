//! Golden staleness snapshot.

mod support;

use serde_json::json;
use support::{ingest_sample_sources, TestResult, TestWorkspace};

#[test]
fn staleness_stale_onboarding() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    let _report = ingest_sample_sources(&mut workspace)?;
    let report = workspace.invoke("texo.staleness.check", &json!({"path": "sample_sources/stale_onboarding.md"}))?;
    insta::assert_json_snapshot!("staleness_stale_onboarding", report, {
        ".diagnostics[].receipt.event_id" => "[event-id]",
        ".diagnostics[].receipt.sequence" => "[sequence]",
        ".replayed_through_sequence" => "[frontier]"
    });
    Ok(())
}
