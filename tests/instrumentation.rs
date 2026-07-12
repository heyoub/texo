//! Deterministic stats and phase instrumentation contracts.

mod support;

use serde_json::json;
use support::{TestResult, TestWorkspace, OBSERVED_AT_MS};

#[test]
fn ingest_verify_and_stats_expose_benchmark_fields() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    workspace.write("docs/policy.md", "Deploys happen on Friday.\n")?;
    let ingest = workspace.invoke(
        "texo.ingest.run",
        &json!({
            "path": "docs",
            "dry_run": false,
            "observed_at_ms": OBSERVED_AT_MS + 1
        }),
    )?;
    assert_eq!(ingest["events_appended"], 2);
    for phase in ["discover", "extract", "append", "project"] {
        assert!(ingest["phase_ms"][phase].is_u64());
    }

    let verify = workspace.invoke("texo.verify.run", &json!({}))?;
    assert!(verify["replay_ms"].is_u64());
    assert!(verify["events_replayed"]
        .as_u64()
        .is_some_and(|count| count >= 3));

    let stats = workspace.invoke("texo.stats.read", &json!({}))?;
    assert_eq!(stats["claims_total"], 1);
    assert!(stats["events_total"]
        .as_u64()
        .is_some_and(|count| count >= 3));
    assert!(stats["journal_bytes"]
        .as_u64()
        .is_some_and(|bytes| bytes > 0));
    assert!(stats["agent_context_bytes"]
        .as_u64()
        .is_some_and(|bytes| bytes > 0));
    assert!(stats["frontier_sequence"]
        .as_u64()
        .is_some_and(|value| value > 0));
    Ok(())
}
