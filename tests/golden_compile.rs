//! Golden compile snapshot.

mod support;

use serde_json::json;
use support::{ingest_sample_sources, TestResult, TestWorkspace, OBSERVED_AT_MS};

#[test]
fn compile_demo() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    let _report = ingest_sample_sources(&mut workspace)?;
    let output = workspace.invoke(
        "texo.compile.run",
        &json!({"out_dir": "public", "observed_at_ms": OBSERVED_AT_MS + 3}),
    )?;
    let onboarding =
        std::fs::read_to_string(workspace.root().join("public/onboarding.generated.md"))?;
    insta::assert_json_snapshot!("compile_demo", output, {
        ".receipt.event_id_hex" => "[event-id]",
        ".receipt.global_sequence" => "[sequence]"
    });
    insta::assert_snapshot!("compile_demo_onboarding", onboarding);
    Ok(())
}
