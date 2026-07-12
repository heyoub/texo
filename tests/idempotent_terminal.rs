//! Terminal-state no-op and mismatch contracts.

mod support;

use serde_json::json;
use support::{TestResult, TestWorkspace, OBSERVED_AT_MS};
use texo::events::coordinate::scope_for_workspace;

#[test]
fn supersede_same_successor_is_noop_and_different_successor_appends_nothing() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    workspace.write("docs/a.md", "Alice owns billing.\n")?;
    workspace.write("docs/b.md", "Deploys happen on Friday.\n")?;
    workspace.write("docs/c.md", "Releases happen on Monday.\n")?;
    let _ = workspace.invoke(
        "texo.ingest.run",
        &json!({"path":"docs", "dry_run":false, "observed_at_ms":OBSERVED_AT_MS + 1}),
    )?;
    let listed = workspace.invoke("texo.claims.list", &json!({"subject": null}))?;
    let ids = listed["claims"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|claim| claim["claim_id"].as_str())
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 3);
    let input = |new: &str| {
        json!({
            "old": ids[0], "new": new, "reason": "test", "decided_by": "human",
            "observed_at_ms": OBSERVED_AT_MS + 2
        })
    };
    let first = workspace.invoke("texo.claim.supersede", &input(&ids[1]))?;
    assert_eq!(first["already_applied"], false);
    let scope = scope_for_workspace("demo");
    let after_first = workspace.host.store().by_scope(&scope).len();

    let repeated = workspace.invoke("texo.claim.supersede", &input(&ids[1]))?;
    assert_eq!(repeated["already_applied"], true);
    assert_eq!(workspace.host.store().by_scope(&scope).len(), after_first);

    let mismatch = workspace
        .invoke("texo.claim.supersede", &input(&ids[2]))
        .expect_err("different successor must fail");
    assert!(mismatch.to_string().contains(&ids[1]));
    assert_eq!(workspace.host.store().by_scope(&scope).len(), after_first);
    Ok(())
}

#[test]
fn conflict_same_resolution_is_noop_and_contrary_resolution_fails() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    workspace.write("docs/monday.md", "Releases happen on Monday.\n")?;
    workspace.write("docs/friday.md", "Releases go out on Friday.\n")?;
    let _ = workspace.invoke(
        "texo.ingest.run",
        &json!({"path":"docs", "dry_run":false, "observed_at_ms":OBSERVED_AT_MS + 1}),
    )?;
    let committed = workspace.invoke(
        "texo.conflicts.commit",
        &json!({"observed_at_ms": OBSERVED_AT_MS + 2}),
    )?;
    let conflict_id = committed[0]["conflict_id"]
        .as_str()
        .ok_or("missing conflict id")?
        .to_string();
    let resolve = |resolution: &str| {
        json!({
            "conflict_id": conflict_id, "resolution": resolution,
            "resolved_by": "human", "observed_at_ms": OBSERVED_AT_MS + 3
        })
    };
    let first = workspace.invoke("texo.conflict.resolve", &resolve("resolved"))?;
    assert_eq!(first["already_applied"], false);
    let scope = scope_for_workspace("demo");
    let after_first = workspace.host.store().by_scope(&scope).len();

    let repeated = workspace.invoke("texo.conflict.resolve", &resolve("resolved"))?;
    assert_eq!(repeated["already_applied"], true);
    assert_eq!(workspace.host.store().by_scope(&scope).len(), after_first);

    let mismatch = workspace
        .invoke("texo.conflict.resolve", &resolve("ignored"))
        .expect_err("contrary resolution must fail");
    assert!(mismatch.to_string().contains("already resolved"));
    assert_eq!(workspace.host.store().by_scope(&scope).len(), after_first);
    Ok(())
}
