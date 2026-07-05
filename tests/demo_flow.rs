//! Demo flow smoke test.

mod support;

use serde_json::json;
use support::{ingest_courtroom, TestResult, TestWorkspace, OBSERVED_AT_MS};

#[test]
fn demo_flow_reaches_verified_compile() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    ingest_courtroom(&mut workspace)?;
    let verify = workspace.invoke("texo.verify.run", &json!({}))?;
    assert_eq!(verify["projection_ok"], true);
    assert_eq!(verify["journal_ok"], true);
    let compile = workspace.invoke(
        "texo.compile.run",
        &json!({"out_dir": "public", "observed_at_ms": OBSERVED_AT_MS + 3}),
    )?;
    assert!(compile["files"]
        .as_array()
        .is_some_and(|files| files.iter().any(|file| file == "onboarding.generated.md")));
    Ok(())
}
