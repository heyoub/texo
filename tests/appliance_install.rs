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
    assert_eq!(first_report["schema"], "texo.install.v1");
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

fn run(root: &std::path::Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new(env!("CARGO_BIN_EXE_texo"))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
}
