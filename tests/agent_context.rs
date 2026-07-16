//! Agent context integration test.

#[path = "support/courtroom.rs"]
mod courtroom_support;
mod support;

use courtroom_support::ingest_courtroom;
use serde_json::json;
use support::{TestResult, TestWorkspace};

#[test]
fn agent_context_contains_current_and_stale_claims() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    ingest_courtroom(&mut workspace)?;
    let context = workspace.invoke(
        "texo.context.agent",
        &json!({"subject": null, "include_stale": true, "allow_unsettled": true}),
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

#[test]
fn snapshot_token_keeps_multi_call_reads_on_one_historical_frontier() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    workspace.write("docs/old.md", "The deploy target is Frankfurt.\n")?;
    let _ = workspace.invoke(
        "texo.ingest.run",
        &json!({
            "path": "docs/old.md",
            "dry_run": false,
            "observed_at_ms": support::OBSERVED_AT_MS + 1
        }),
    )?;
    let first = workspace.invoke(
        "texo.context.agent",
        &json!({"subject": null, "include_stale": true, "allow_unsettled": true}),
    )?;
    let token = first["snapshot"]["token"]
        .as_str()
        .ok_or("context snapshot token")?
        .to_string();
    let first_frontier = first["replayed_through_sequence"]
        .as_u64()
        .ok_or("first frontier")?;

    workspace.write(
        "docs/new.md",
        "Decision: the deploy target moved to Singapore.\n",
    )?;
    let _ = workspace.invoke(
        "texo.ingest.run",
        &json!({
            "path": "docs/new.md",
            "dry_run": false,
            "observed_at_ms": support::OBSERVED_AT_MS + 2
        }),
    )?;

    let historical = workspace.invoke(
        "texo.context.agent",
        &json!({
            "subject": null,
            "include_stale": true,
            "snapshot": token,
            "allow_unsettled": true
        }),
    )?;
    let latest = workspace.invoke(
        "texo.context.agent",
        &json!({"subject": null, "include_stale": true, "allow_unsettled": true}),
    )?;

    assert_eq!(historical["replayed_through_sequence"], first_frontier);
    assert_eq!(historical["claims"], first["claims"]);
    assert_eq!(historical["stale_claims"], first["stale_claims"]);
    assert!(latest["replayed_through_sequence"]
        .as_u64()
        .is_some_and(|frontier| frontier > first_frontier));
    assert_ne!(latest["claims"], historical["claims"]);
    Ok(())
}

#[test]
fn tampered_snapshot_token_fails_with_snapshot_facts() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    let error = workspace
        .invoke(
            "texo.context.agent",
            &json!({
                "subject": null,
                "include_stale": false,
                "snapshot": "texo_snap_v2.1.deadbeef.none.deadbeef"
            }),
        )
        .expect_err("tampered token must fail");
    assert_eq!(error.code(), "snapshot.invalid");
    assert_eq!(error.facts().committed, texo::error::Committed::No);
    assert!(error.facts().retry_safe);
    Ok(())
}
