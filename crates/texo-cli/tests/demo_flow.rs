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
