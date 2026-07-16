//! End-to-end pipeline smoke test.

#[path = "support/courtroom.rs"]
mod courtroom_support;
mod support;

use courtroom_support::ingest_courtroom;
use serde_json::json;
use support::{TestResult, TestWorkspace};

#[test]
fn e2e_pipeline_lists_explains_and_checks_staleness() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    ingest_courtroom(&mut workspace)?;
    let claims = workspace.invoke("texo.claims.list", &json!({"subject": null}))?;
    let stale = claims["claims"]
        .as_array()
        .expect("claims array")
        .iter()
        .find(|claim| claim["status"] == "superseded")
        .and_then(|claim| claim["claim_id"].as_str())
        .expect("superseded claim")
        .to_string();
    let explain = workspace.invoke("texo.claim.explain", &json!({"claim_id": stale}))?;
    assert!(explain["timeline"]
        .as_array()
        .is_some_and(|timeline| timeline.len() >= 2));
    let stale_report = workspace.invoke("texo.staleness.check", &json!({"path": "docs"}))?;
    assert!(stale_report["diagnostics"]
        .as_array()
        .is_some_and(|diagnostics| !diagnostics.is_empty()));
    Ok(())
}
