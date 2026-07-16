//! Demo-corpus fixture used only by integration tests that request it.

use std::path::Path;

use serde_json::{json, Value};

use crate::support::{TestResult, TestWorkspace, OBSERVED_AT_MS};

fn copy_sample_sources(workspace: &TestWorkspace) -> TestResult {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("sample_sources");
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let relative = format!("sample_sources/{}", entry.file_name().to_string_lossy());
        let text = std::fs::read_to_string(entry.path())?;
        workspace.write(&relative, &text)?;
    }
    Ok(())
}

/// Copy and ingest the bundled deterministic demo corpus.
pub fn ingest_sample_sources(workspace: &mut TestWorkspace) -> TestResult<Value> {
    copy_sample_sources(workspace)?;
    Ok(workspace.invoke(
        "texo.ingest.run",
        &json!({"path": "sample_sources", "dry_run": false, "observed_at_ms": OBSERVED_AT_MS}),
    )?)
}
