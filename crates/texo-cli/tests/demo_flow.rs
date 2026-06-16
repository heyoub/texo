//! End-to-end demo flow via CLI.

use assert_cmd::Command;
use texo_core::fixture::FIXTURE_OBSERVED_AT_MS;

fn texo() -> Command {
    let mut cmd = Command::cargo_bin("texo").expect("texo binary");
    cmd.env("TEXO_OBSERVED_AT_MS", FIXTURE_OBSERVED_AT_MS.to_string());
    cmd
}

#[test]
fn demo_flow_stale_onboarding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    for file in [
        "old_process.md",
        "meeting_notes.md",
        "stale_onboarding.md",
        "current_architecture.md",
    ] {
        let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../sample_sources")
            .join(file);
        let dest = root.join("sample_sources").join(file);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(src, dest).unwrap();
    }

    texo()
        .args(["init", "--workspace", "demo"])
        .current_dir(root)
        .assert()
        .success();
    texo()
        .args(["ingest", "sample_sources"])
        .current_dir(root)
        .assert()
        .success();
    texo()
        .args([
            "check-staleness",
            "sample_sources/stale_onboarding.md",
            "--json",
        ])
        .current_dir(root)
        .assert()
        .success()
        .stdout(predicates::str::contains("superseded"));
    texo()
        .args(["agent-context", "--out", "public/agent-context.json"])
        .current_dir(root)
        .assert()
        .success();
    texo()
        .args(["compile", "--out", "public"])
        .current_dir(root)
        .assert()
        .success();
}

/// `--json` must emit JSON to stdout, distinctly from the default file-only
/// behavior of `--out`. With both flags set the file is written AND JSON is
/// printed to stdout; with neither, JSON goes to stdout.
#[test]
fn agent_context_json_flag_controls_stdout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../sample_sources")
        .join("current_architecture.md");
    let dest = root.join("sample_sources").join("current_architecture.md");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::copy(src, dest).unwrap();

    texo()
        .args(["init", "--workspace", "demo"])
        .current_dir(root)
        .assert()
        .success();
    texo()
        .args(["ingest", "sample_sources"])
        .current_dir(root)
        .assert()
        .success();

    // --out without --json writes the file and prints nothing to stdout.
    let out_only = texo()
        .args(["agent-context", "--out", "public/ctx-a.json"])
        .current_dir(root)
        .output()
        .expect("run");
    assert!(out_only.status.success());
    assert!(
        out_only.stdout.is_empty(),
        "default --out must not print to stdout: {:?}",
        String::from_utf8_lossy(&out_only.stdout)
    );
    assert!(root.join("public/ctx-a.json").is_file());

    // --out together with --json writes the file AND prints JSON to stdout.
    let both = texo()
        .args(["agent-context", "--out", "public/ctx-b.json", "--json"])
        .current_dir(root)
        .output()
        .expect("run");
    assert!(both.status.success());
    assert!(root.join("public/ctx-b.json").is_file());
    let parsed: serde_json::Value =
        serde_json::from_slice(&both.stdout).expect("--json must print valid JSON to stdout");
    assert!(
        parsed.get("claims").is_some(),
        "expected agent-context JSON"
    );
}
