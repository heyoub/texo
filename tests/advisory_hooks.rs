//! Fixed advisory hook command contracts.

use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn fixed_hooks_return_machine_readable_advisory_results() -> TestResult {
    let root = TempDir::new()?;
    assert!(run(root.path(), &["init", "--workspace", "demo"])?
        .status
        .success());

    for (command, event) in [
        ("session-start", "session_start"),
        ("pre-commit", "pre_commit"),
    ] {
        let output = run(root.path(), &["hook", command, "--json"])?;
        assert!(output.status.success());
        let result: Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(result["schema"], "texo.hook-result.v1");
        assert_eq!(result["event"], event);
        assert_eq!(result["advisory"], true);
    }
    Ok(())
}

#[test]
fn files_changed_accepts_only_bounded_relative_paths() -> TestResult {
    let root = TempDir::new()?;
    assert!(run(root.path(), &["init", "--workspace", "demo"])?
        .status
        .success());
    std::fs::write(root.path().join("README.md"), "# Local\n")?;

    let mut child = Command::new(env!("CARGO_BIN_EXE_texo"))
        .arg("--root")
        .arg(root.path())
        .args(["hook", "files-changed", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("stdin")?;
    std::io::Write::write_all(&mut stdin, br#"{"paths":["README.md"]}"#)?;
    drop(stdin);
    let output = child.wait_with_output()?;
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        result["data"]["reports"].as_array().ok_or("reports")?.len(),
        1
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_texo"))
        .arg("--root")
        .arg(root.path())
        .args(["hook", "files-changed", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("stdin")?;
    std::io::Write::write_all(&mut stdin, br#"{"paths":["../outside"]}"#)?;
    drop(stdin);
    let output = child.wait_with_output()?;
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)?.contains("may not escape"));
    Ok(())
}

fn run(root: &std::path::Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new(env!("CARGO_BIN_EXE_texo"))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
}
