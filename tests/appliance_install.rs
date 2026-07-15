//! Agent-client installation and removal contracts.

use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn cli_install_all_is_repeatable_and_uninstall_keeps_workspace() -> TestResult {
    let root = TempDir::new()?;
    std::fs::write(root.path().join("AGENTS.md"), "# Existing guidance\n")?;

    let first = run(
        root.path(),
        &[
            "--workspace",
            "demo",
            "install",
            "--client",
            "all",
            "--json",
        ],
    )?;
    assert!(first.status.success());
    let first_report: Value = serde_json::from_slice(&first.stdout)?;
    assert_eq!(first_report["schema"], "texo.install.v2");
    assert!(root.path().join(".texo/mcp.json").is_file());
    let hooks: Value =
        serde_json::from_slice(&std::fs::read(root.path().join(".texo/hooks.json"))?)?;
    assert_eq!(hooks["schema"], "texo.hooks.v1");
    assert_eq!(hooks["hooks"].as_array().ok_or("hooks")?.len(), 3);
    assert!(root.path().join(".codex/config.toml").is_file());
    assert!(root.path().join(".mcp.json").is_file());
    assert!(root.path().join(".cursor/mcp.json").is_file());

    let second = run(
        root.path(),
        &[
            "--workspace",
            "demo",
            "install",
            "--client",
            "all",
            "--json",
        ],
    )?;
    assert!(second.status.success());
    let second_report: Value = serde_json::from_slice(&second.stdout)?;
    assert!(second_report["changes"]
        .as_array()
        .ok_or("changes")?
        .iter()
        .all(|change| change["action"] == "unchanged"));

    let removed = run(root.path(), &["uninstall", "--json"])?;
    assert!(removed.status.success());
    assert!(root.path().join(".texo/config.toml").is_file());
    assert!(std::fs::read_to_string(root.path().join("AGENTS.md"))?.contains("# Existing guidance"));
    Ok(())
}

#[test]
fn cli_dry_run_does_not_create_a_workspace() -> TestResult {
    let root = TempDir::new()?;
    let output = run(
        root.path(),
        &["install", "--client", "all", "--dry-run", "--json"],
    )?;
    assert!(output.status.success());
    assert!(std::fs::read_dir(root.path())?.next().is_none());
    Ok(())
}

#[test]
fn cli_targeted_uninstall_is_symmetric_with_install() -> TestResult {
    let root = TempDir::new()?;
    assert!(run(root.path(), &["install", "--client", "all", "--json"])?
        .status
        .success());

    let removed = run(root.path(), &["uninstall", "--client", "cursor", "--json"])?;

    assert!(removed.status.success());
    let report: Value = serde_json::from_slice(&removed.stdout)?;
    assert_eq!(report["clients"][0], "cursor");
    assert!(!root.path().join(".cursor/mcp.json").exists());
    assert!(root.path().join(".mcp.json").is_file());
    assert!(root.path().join(".codex/config.toml").is_file());
    assert!(root.path().join(".texo/mcp.json").is_file());
    Ok(())
}

fn run(root: &std::path::Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new(env!("CARGO_BIN_EXE_texo"))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
}

#[test]
fn uninstall_fails_closed_on_unrecoverable_workspace_manifest() -> TestResult {
    // A managed manifest with the correct schema but an args shape that does
    // not carry a recoverable `--workspace <id>` pair must NOT be silently
    // rewritten to point clients at the `demo` workspace — it must fail closed.
    let root = TempDir::new()?;
    run(
        root.path(),
        &["--workspace", "real-ws", "install", "--client", "cursor"],
    )?;

    let manifest_path = root.path().join(".texo/mcp.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
    // Valid schema, but the args no longer contain a --workspace flag pair.
    manifest["server"]["args"] = serde_json::json!(["mcp"]);
    let before = serde_json::to_vec_pretty(&manifest)?;
    std::fs::write(&manifest_path, &before)?;

    let removed = run(root.path(), &["uninstall", "--client", "cursor", "--json"])?;
    assert!(
        !removed.status.success(),
        "uninstall must fail closed on an unrecoverable managed manifest"
    );

    // The manifest must be untouched — no silent rewrite to `demo`.
    let after = std::fs::read(&manifest_path)?;
    assert_eq!(after, before, "manifest was mutated despite failing closed");
    let reparsed: Value = serde_json::from_slice(&after)?;
    assert_ne!(
        reparsed["server"]["args"],
        serde_json::json!(["--root", ".", "--workspace", "demo", "mcp"]),
        "manifest was rewritten to the demo fallback"
    );
    Ok(())
}
