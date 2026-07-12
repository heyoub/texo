//! Composed doctor status and safe-repair contracts.

use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[test]
fn doctor_distinguishes_broken_degraded_and_healthy() -> TestResult {
    let root = TempDir::new()?;
    let missing = doctor(root.path(), &[])?;
    assert!(!missing.0);
    assert_eq!(missing.1["status"], "broken");
    assert!(has_check(&missing.1, "config.parse", "fail"));

    assert!(run(root.path(), &["init", "--workspace", "demo"])?
        .status
        .success());
    assert!(run(root.path(), &["install", "--client", "all", "--json"])?
        .status
        .success());
    let healthy = doctor(root.path(), &["--deep"])?;
    assert!(healthy.0);
    assert_eq!(healthy.1["status"], "healthy");
    assert!(has_check(&healthy.1, "workspace.verify", "pass"));

    std::fs::remove_file(root.path().join(".texo/hooks.json"))?;
    let degraded = doctor(root.path(), &[])?;
    assert!(degraded.0);
    assert_eq!(degraded.1["status"], "degraded");
    assert!(has_check(&degraded.1, "integration.managed", "warn"));

    let fixed = doctor(root.path(), &["--fix"])?;
    assert!(fixed.0);
    assert_eq!(fixed.1["status"], "healthy");
    assert!(root.path().join(".texo/hooks.json").is_file());
    Ok(())
}

#[test]
fn doctor_reports_heuristic_readiness_without_exposing_secrets() -> TestResult {
    let root = TempDir::new()?;
    assert!(run(root.path(), &["init", "--workspace", "demo"])?
        .status
        .success());
    assert!(run(root.path(), &["install", "--json"])?.status.success());

    let report = doctor(root.path(), &[])?.1;
    let encoded = serde_json::to_string(&report)?;
    assert!(encoded.contains("heuristic-only mode"));
    assert!(!encoded.contains("api_key"));
    Ok(())
}

#[test]
fn doctor_fix_never_overwrites_malformed_user_config() -> TestResult {
    let root = TempDir::new()?;
    std::fs::create_dir_all(root.path().join(".texo"))?;
    let malformed = b"this is not = valid toml [";
    std::fs::write(root.path().join(".texo/config.toml"), malformed)?;

    let report = doctor(root.path(), &["--fix"])?;

    assert!(!report.0);
    assert_eq!(report.1["status"], "broken");
    assert_eq!(
        std::fs::read(root.path().join(".texo/config.toml"))?,
        malformed
    );
    Ok(())
}

fn doctor(root: &std::path::Path, args: &[&str]) -> TestResult<(bool, Value)> {
    let mut command = vec!["doctor", "--json"];
    command.extend_from_slice(args);
    let output = run(root, &command)?;
    Ok((
        output.status.success(),
        serde_json::from_slice(&output.stdout)?,
    ))
}

fn has_check(report: &Value, id: &str, status: &str) -> bool {
    report["checks"].as_array().is_some_and(|checks| {
        checks
            .iter()
            .any(|check| check["id"] == id && check["status"] == status)
    })
}

fn run(root: &std::path::Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new(env!("CARGO_BIN_EXE_texo"))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
}
