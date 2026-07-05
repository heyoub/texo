//! CLI golden snapshot.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const OBSERVED_AT_MS: u64 = 1_700_000_000_000;

#[test]
fn claims_json() -> TestResult {
    let dir = TempDir::new()?;
    std::fs::create_dir_all(dir.path().join("docs"))?;
    std::fs::write(
        dir.path().join("docs/friday.md"),
        "Deploys happen on Friday.\n",
    )?;
    std::fs::write(
        dir.path().join("docs/tuesday.md"),
        "Decision: deploys moved to Tuesday.\n",
    )?;

    texo_cmd()?
        .arg("--root")
        .arg(dir.path())
        .args(["init", "--workspace", "demo"])
        .assert()
        .success();
    texo_cmd()?
        .arg("--root")
        .arg(dir.path())
        .args(["ingest", "docs/friday.md"])
        .env("TEXO_OBSERVED_AT_MS", OBSERVED_AT_MS.to_string())
        .assert()
        .success();
    texo_cmd()?
        .arg("--root")
        .arg(dir.path())
        .args(["ingest", "docs/tuesday.md"])
        .env(
            "TEXO_OBSERVED_AT_MS",
            OBSERVED_AT_MS.saturating_add(1).to_string(),
        )
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
