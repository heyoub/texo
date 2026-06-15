//! Integration tests for MCP tool handlers (real BatPak replay path).

use std::path::Path;

use texo_core::{ingest_sources, init_workspace, open_journal, IngestMode, FIXTURE_OBSERVED_AT_MS};
use texo_mcp::tools::{GetAgentContextInput, ToolContext};

fn repo_sample_sources() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sample_sources")
}

fn copy_sample_sources(root: &Path) {
    let dest = root.join("sample_sources");
    std::fs::create_dir_all(&dest).expect("mkdir sample_sources");
    for entry in std::fs::read_dir(repo_sample_sources()).expect("read sample_sources") {
        let entry = entry.expect("entry");
        std::fs::copy(entry.path(), dest.join(entry.file_name())).expect("copy sample");
    }
}

fn setup_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    init_workspace(dir.path(), "demo").expect("init");
    copy_sample_sources(dir.path());
    let journal = open_journal(dir.path()).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    ingest_sources(
        journal.handle(),
        journal.config(),
        &workspace,
        &dir.path().join("sample_sources"),
        IngestMode::Commit,
        FIXTURE_OBSERVED_AT_MS,
    )
    .expect("ingest");
    journal.close().expect("close");
    dir
}

#[test]
fn get_agent_context_returns_current_claims() {
    let dir = setup_workspace();
    let ctx = ToolContext {
        root: dir.path().to_path_buf(),
    };
    let json = ctx
        .get_agent_context(&GetAgentContextInput {
            subject_hint: None,
            include_stale: true,
        })
        .expect("agent context");
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert!(value["replayed_through_sequence"].as_u64().unwrap_or(0) > 0);
    assert!(!value["claims"].as_array().unwrap_or(&vec![]).is_empty());
}

#[test]
fn check_staleness_flags_stale_onboarding() {
    let dir = setup_workspace();
    let ctx = ToolContext {
        root: dir.path().to_path_buf(),
    };
    let json = ctx
        .check_staleness(&texo_mcp::tools::CheckStalenessInput {
            path: "sample_sources/stale_onboarding.md".to_string(),
        })
        .expect("staleness");
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert!(value["diagnostics"]
        .as_array()
        .is_some_and(|d| !d.is_empty()));
}
