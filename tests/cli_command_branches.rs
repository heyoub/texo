//! CLI command branch smoke tests.

use assert_cmd::Command;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn host_fingerprint_branch_is_wired() -> TestResult {
    let dir = TempDir::new()?;
    Command::cargo_bin("texo")?
        .arg("--root")
        .arg(dir.path())
        .args(["host", "fingerprint"])
        .assert()
        .success();
    Ok(())
}

#[test]
#[ignore = "WO-6"]
fn mcp_branch_is_not_wired_until_wo6() -> TestResult {
    let dir = TempDir::new()?;
    Command::cargo_bin("texo")?
        .arg("--root")
        .arg(dir.path())
        .arg("mcp")
        .assert()
        .failure();
    Ok(())
}
