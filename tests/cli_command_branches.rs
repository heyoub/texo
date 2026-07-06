//! CLI command branch smoke tests.

use assert_cmd::prelude::*;
use assert_cmd::Command as AssertCommand;
use std::io::Write;
use std::process::Command;
use std::process::Stdio;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn host_fingerprint_branch_is_wired() -> TestResult {
    let dir = TempDir::new()?;
    AssertCommand::cargo_bin("texo")?
        .arg("--root")
        .arg(dir.path())
        .args(["host", "fingerprint"])
        .assert()
        .success();
    Ok(())
}

#[test]
fn mcp_branch_initialize_exits_cleanly_on_eof() -> TestResult {
    let dir = TempDir::new()?;
    let mut command = Command::cargo_bin("texo")?;
    let mut child = command
        .arg("--root")
        .arg(dir.path())
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("child stdin is piped");
    stdin.write_all(
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"test"}}"#,
    )?;
    stdin.write_all(b"\n")?;
    drop(stdin);
    let output = child.wait_with_output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains(r#""name":"texo""#));
    Ok(())
}
