//! Source-settlement and strict/tolerant ingest contracts.

mod support;

use std::process::Command;

use serde_json::json;
use support::{TestResult, TestWorkspace, OBSERVED_AT_MS};
use texo::events::coordinate::scope_for_workspace;

fn write_fixture(workspace: &TestWorkspace) -> TestResult {
    workspace.write("docs/a.md", "Alice owns release approval.\n")?;
    workspace.write("docs/b.md", "Deploys happen on Friday.\n")?;
    std::fs::write(workspace.dir.path().join("docs/bad.md"), [0xff, 0xfe])?;
    Ok(())
}

#[test]
fn tolerant_records_skips_but_strict_appends_nothing() -> TestResult {
    let mut tolerant = TestWorkspace::new()?;
    write_fixture(&tolerant)?;
    let output = tolerant.invoke(
        "texo.ingest.run",
        &json!({
            "path": "docs",
            "dry_run": false,
            "strict": false,
            "observed_at_ms": OBSERVED_AT_MS + 1
        }),
    )?;
    assert_eq!(output["outcome"], "partial");
    assert_eq!(output["sources_observed"], 2);
    assert_eq!(output["skipped"][0]["code"], "source.utf8");
    let claims = tolerant.invoke("texo.claims.list", &json!({"subject": null}))?;
    assert_eq!(claims["claims"].as_array().map_or(0, Vec::len), 2);

    let mut strict = TestWorkspace::new()?;
    write_fixture(&strict)?;
    let scope = scope_for_workspace("demo");
    let before = strict.host.store().by_scope(&scope).len();
    let error = strict
        .invoke(
            "texo.ingest.run",
            &json!({
                "path": "docs",
                "dry_run": false,
                "strict": true,
                "observed_at_ms": OBSERVED_AT_MS + 1
            }),
        )
        .expect_err("strict planning failure");
    assert!(error.to_string().contains("source"));
    assert_eq!(strict.host.store().by_scope(&scope).len(), before);
    Ok(())
}

#[test]
fn missing_root_fails_and_existing_empty_root_is_stamped_success() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    let missing = workspace
        .invoke(
            "texo.ingest.run",
            &json!({
                "path": "missing",
                "dry_run": false,
                "observed_at_ms": OBSERVED_AT_MS + 1
            }),
        )
        .expect_err("missing root must fail");
    assert!(missing.to_string().contains("source"));

    std::fs::create_dir_all(workspace.dir.path().join("empty"))?;
    let empty = workspace.invoke(
        "texo.ingest.run",
        &json!({
            "path": "empty",
            "dry_run": false,
            "observed_at_ms": OBSERVED_AT_MS + 2
        }),
    )?;
    assert_eq!(empty["outcome"], "complete");
    assert_eq!(empty["empty"], true);
    assert_eq!(empty["sources_observed"], 0);
    Ok(())
}

#[test]
fn repeated_init_is_an_explicit_event_free_noop() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    let scope = scope_for_workspace("demo");
    let before = workspace.host.store().by_scope(&scope).len();
    let output = workspace.invoke("texo.workspace.init", &json!({"workspace_id": "demo"}))?;
    assert_eq!(output["already_initialized"], true);
    assert_eq!(workspace.host.store().by_scope(&scope).len(), before);
    Ok(())
}

#[test]
fn cli_uses_tristate_exit_for_tolerant_partial() -> TestResult {
    let root = tempfile::tempdir()?;
    let bin = env!("CARGO_BIN_EXE_texo");
    let root_arg = root.path().to_string_lossy().to_string();
    let init = Command::new(bin)
        .args(["--root", root_arg.as_str(), "init"])
        .output()?;
    assert!(init.status.success());
    std::fs::create_dir_all(root.path().join("docs"))?;
    std::fs::write(
        root.path().join("docs/good.md"),
        "Deploys happen on Friday.\n",
    )?;
    std::fs::write(root.path().join("docs/bad.md"), [0xff, 0xfe])?;

    let tolerant = Command::new(bin)
        .args(["--root", root_arg.as_str(), "ingest", "docs", "--json"])
        .output()?;
    assert_eq!(tolerant.status.code(), Some(2));
    let output: serde_json::Value = serde_json::from_slice(&tolerant.stdout)?;
    assert_eq!(output["outcome"], "partial");

    let missing = Command::new(bin)
        .args(["--root", root_arg.as_str(), "ingest", "missing"])
        .output()?;
    assert_eq!(missing.status.code(), Some(1));
    let stderr = String::from_utf8(missing.stderr)?;
    assert!(stderr.contains("error[op.runtime]"));
    assert!(stderr.contains("committed: no"));
    assert!(stderr.contains("retry: unsafe"));
    Ok(())
}
