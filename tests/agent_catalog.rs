//! Agent catalog discovery, pagination, and workspace-status contracts.

mod support;

use serde_json::json;
use support::{ingest_courtroom, TestResult, TestWorkspace};

#[test]
fn cli_operation_discovery_uses_the_shared_catalog() -> TestResult {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_texo"))
        .args(["ops", "list", "--json"])
        .output()?;
    assert!(output.status.success());
    let inventory: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(inventory["schema"], "texo.agent-catalog.v1");
    let operations = inventory["operations"]
        .as_array()
        .ok_or("operations array")?;
    assert!(operations.iter().any(|operation| {
        operation["name"] == "texo.claims.search" && operation["agent_tool"] == "search_claims"
    }));
    Ok(())
}

#[test]
fn bounded_claim_search_pages_deterministically_and_status_is_explicit() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    ingest_courtroom(&mut workspace)?;

    let first = workspace.invoke(
        "texo.claims.search",
        &json!({
            "query": null,
            "subject": null,
            "status": null,
            "limit": 1,
            "cursor": null
        }),
    )?;
    assert_eq!(first["returned"], 1);
    assert_eq!(first["has_more"], true);
    let cursor = first["next_cursor"]
        .as_str()
        .ok_or("first search page must carry a cursor")?;

    let second = workspace.invoke(
        "texo.claims.search",
        &json!({
            "query": null,
            "subject": null,
            "status": null,
            "limit": 1,
            "cursor": cursor
        }),
    )?;
    assert_ne!(
        first["claims"][0]["claim_id"], second["claims"][0]["claim_id"],
        "cursor must advance in stable claim-id order"
    );

    let status = workspace.invoke("texo.workspace.status", &json!({}))?;
    assert_eq!(status["workspace_id"], "demo");
    assert!(status["frontier"].as_u64().is_some());
    assert!(status["freshness"].is_string());
    assert!(status["settlement_complete"].is_boolean());
    Ok(())
}

#[test]
fn claim_search_rejects_unbounded_or_forged_pagination() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    let too_large = workspace
        .invoke(
            "texo.claims.search",
            &json!({"query": null, "subject": null, "status": null, "limit": 101, "cursor": null}),
        )
        .expect_err("oversized page must fail");
    assert_eq!(too_large.code(), "op.runtime");
    assert!(too_large.to_string().contains("limit must be between"));

    let forged = workspace
        .invoke(
            "texo.claims.search",
            &json!({"query": null, "subject": null, "status": null, "limit": 25, "cursor": "other:v1:2"}),
        )
        .expect_err("foreign cursor must fail");
    assert_eq!(forged.code(), "op.runtime");
    assert!(forged.to_string().contains("unsupported schema"));
    Ok(())
}
