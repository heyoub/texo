//! Golden snapshot for `texo claims --json`.

use assert_cmd::cargo::cargo_bin;
use assert_cmd::Command;
use insta::assert_json_snapshot;
use serde_json::Value;
use tempfile::TempDir;
use texo_core::fixture::FIXTURE_OBSERVED_AT_MS;

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn setup_workspace() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = repo_root();
    let sample = root.join("sample_sources");
    let dest = dir.path().join("sample_sources");
    std::fs::create_dir_all(&dest).expect("mkdir");
    for entry in std::fs::read_dir(&sample).expect("read sample_sources") {
        let entry = entry.expect("entry");
        std::fs::copy(entry.path(), dest.join(entry.file_name())).expect("copy");
    }

    Command::new(cargo_bin("texo"))
        .args(["init", "--workspace", "demo"])
        .env("TEXO_OBSERVED_AT_MS", FIXTURE_OBSERVED_AT_MS.to_string())
        .current_dir(dir.path())
        .assert()
        .success();
    Command::new(cargo_bin("texo"))
        .args(["ingest", "sample_sources"])
        .env("TEXO_OBSERVED_AT_MS", FIXTURE_OBSERVED_AT_MS.to_string())
        .current_dir(dir.path())
        .assert()
        .success();
    dir
}

fn claims_summary(value: &Value) -> serde_json::Value {
    let claims = value.as_array().expect("claims array");
    serde_json::json!({
        "count": claims.len(),
        "subjects": claims.iter().map(|c| c["subject_hint"].as_str().unwrap_or("")).collect::<Vec<_>>(),
        "statuses": claims.iter().map(|c| c["status"].as_str().unwrap_or("")).collect::<Vec<_>>(),
        "sequences": claims.iter().map(|c| c["receipt"]["sequence"].as_u64().unwrap_or(0)).collect::<Vec<_>>(),
    })
}

#[test]
fn claims_json_golden() {
    let dir = setup_workspace();
    let output = Command::new(cargo_bin("texo"))
        .args(["claims", "--json"])
        .env("TEXO_OBSERVED_AT_MS", FIXTURE_OBSERVED_AT_MS.to_string())
        .current_dir(dir.path())
        .output()
        .expect("run claims");
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_json_snapshot!("claims_json", claims_summary(&value));
}
