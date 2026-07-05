//! Compile journaling integration test.

mod support;

use serde_json::json;
use support::{ingest_courtroom, TestResult, TestWorkspace, OBSERVED_AT_MS};

#[test]
fn compile_writes_outputs_and_receipt() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    ingest_courtroom(&mut workspace)?;
    let output = workspace.invoke(
        "texo.compile.run",
        &json!({"out_dir": "public", "observed_at_ms": OBSERVED_AT_MS + 3}),
    )?;
    assert!(output.get("receipt").is_some());
    for name in [
        "onboarding.generated.md",
        "claims.json",
        "stale-context.json",
        "conflicts.json",
        "agent-context.json",
        "index.html",
    ] {
        assert!(workspace.root().join("public").join(name).exists());
    }
    Ok(())
}
