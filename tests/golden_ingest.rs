//! Golden ingest snapshot.

mod support;

use serde_json::json;
use support::{ingest_sample_sources, TestResult, TestWorkspace};

#[test]
fn ingest_demo() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    let _report = ingest_sample_sources(&mut workspace)?;
    let claims = workspace.invoke("texo.claims.list", &json!({"subject": null}))?;
    insta::assert_json_snapshot!("ingest_demo", claims, {
        ".claims[].receipt.event_id" => "[event-id]",
        ".claims[].receipt.sequence" => "[sequence]",
        ".frontier" => "[frontier]"
    });
    Ok(())
}
