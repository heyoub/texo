//! CLI golden snapshot.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const OBSERVED_AT_MS: u64 = 1_700_000_000_000;

#[test]
fn claims_json() -> TestResult {
    let dir = TempDir::new()?;
    // The bundled demo corpus — the same fixture the pre-v2 golden used.
    let sample = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sample_sources");
    let dest = dir.path().join("sample_sources");
    std::fs::create_dir_all(&dest)?;
    for entry in std::fs::read_dir(&sample)? {
        let entry = entry?;
        std::fs::copy(entry.path(), dest.join(entry.file_name()))?;
    }

    texo_cmd()?
        .arg("--root")
        .arg(dir.path())
        .args(["init", "--workspace", "demo"])
        .assert()
        .success();
    texo_cmd()?
        .arg("--root")
        .arg(dir.path())
        .args(["ingest", "sample_sources"])
        .env("TEXO_OBSERVED_AT_MS", OBSERVED_AT_MS.to_string())
        .assert()
        .success();

    let assert = texo_cmd()?
        .arg("--root")
        .arg(dir.path())
        .args(["claims", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Deploys happen on Friday"));
    let stdout = String::from_utf8(assert.get_output().stdout.clone())?;
    let value: serde_json::Value = serde_json::from_str(&stdout)?;
    insta::assert_json_snapshot!("claims_json", value, {
        "[].receipt.event_id" => "[event-id]",
        "[].receipt.sequence" => "[sequence]"
    });
    Ok(())
}

fn texo_cmd() -> Result<Command, assert_cmd::cargo::CargoError> {
    Command::cargo_bin("texo")
}
