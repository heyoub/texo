//! Integration tests for MCP tool handlers (real BatPak replay path).

use std::path::Path;

use texo_core::{ingest_sources, init_workspace, open_journal, IngestMode, FIXTURE_OBSERVED_AT_MS};
use texo_mcp::error::McpToolError;
use texo_mcp::tools::{
    CheckStalenessInput, ExplainClaimInput, GetAgentContextInput, GetCurrentClaimsInput,
    ToolContext,
};

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
        dir.path(),
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
        workspace_id: None,
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
fn get_current_claims_returns_claims_and_frontier() {
    // PROVES: F10 — get_current_claims output carries BOTH a `claims` array and
    // the `replayed_through_sequence` frontier field.
    let dir = setup_workspace();
    let ctx = ToolContext {
        root: dir.path().to_path_buf(),
        workspace_id: None,
    };
    let json = ctx
        .get_current_claims(&GetCurrentClaimsInput { subject_hint: None })
        .expect("current claims");
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");

    assert!(
        value.get("claims").and_then(|c| c.as_array()).is_some(),
        "F10: get_current_claims must return a `claims` array, got: {value}"
    );
    assert!(
        !value["claims"].as_array().unwrap_or(&vec![]).is_empty(),
        "F10: demo ingest must yield at least one current claim"
    );
    assert!(
        value.get("replayed_through_sequence").is_some(),
        "F10: get_current_claims must include the `replayed_through_sequence` frontier"
    );
    assert!(
        value["replayed_through_sequence"].as_u64().unwrap_or(0) > 0,
        "F10: replayed_through_sequence frontier must advance past zero, got: {value}"
    );
}

#[test]
fn check_staleness_flags_stale_onboarding() {
    let dir = setup_workspace();
    let ctx = ToolContext {
        root: dir.path().to_path_buf(),
        workspace_id: None,
    };
    let json = ctx
        .check_staleness(&CheckStalenessInput {
            path: "sample_sources/stale_onboarding.md".to_string(),
        })
        .expect("staleness");
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert!(value["diagnostics"]
        .as_array()
        .is_some_and(|d| !d.is_empty()));
}

/// Pull a current claim id out of `get_current_claims` so we can explain it.
fn first_current_claim_id(ctx: &ToolContext) -> String {
    let json = ctx
        .get_current_claims(&GetCurrentClaimsInput { subject_hint: None })
        .expect("current claims");
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    value["claims"][0]["claim_id"]
        .as_str()
        .expect("a current claim id")
        .to_string()
}

#[test]
fn explain_claim_returns_full_provenance_for_known_claim() {
    let dir = setup_workspace();
    let ctx = ToolContext {
        root: dir.path().to_path_buf(),
        workspace_id: None,
    };
    let claim_id = first_current_claim_id(&ctx);

    let json = ctx
        .explain_claim(&ExplainClaimInput {
            claim_id: claim_id.clone(),
        })
        .expect("explain");
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");

    assert_eq!(
        value["claim_id"].as_str(),
        Some(claim_id.as_str()),
        "explanation must echo the requested claim id: {value}"
    );
    assert_eq!(
        value["status"].as_str(),
        Some("current"),
        "the explained claim must be current: {value}"
    );
    assert!(
        value["source"]["path"]
            .as_str()
            .is_some_and(|p| !p.is_empty()),
        "explanation must carry source provenance: {value}"
    );
    assert!(
        value["receipt"]["sequence"].as_u64().unwrap_or(0) > 0,
        "explanation must carry a sequenced receipt: {value}"
    );
    assert!(
        value["replayed_through_sequence"].as_u64().unwrap_or(0) > 0,
        "explanation must carry the replay frontier: {value}"
    );
}

#[test]
fn explain_claim_unknown_id_is_domain_error() {
    // PROVES: an unknown-but-well-formed claim id surfaces a typed domain error
    // (not a panic, not an Ok), exercising the `ok_or_else` branch in
    // ToolContext::explain_claim and the TexoError -> McpToolError path.
    let dir = setup_workspace();
    let ctx = ToolContext {
        root: dir.path().to_path_buf(),
        workspace_id: None,
    };
    let err = ctx
        .explain_claim(&ExplainClaimInput {
            // Well-formed `claim_` prefix so ClaimId parsing succeeds, but no
            // such claim exists in the replayed state.
            claim_id: "claim_ffffffffffff".to_string(),
        })
        .expect_err("unknown claim must error");
    assert!(
        err.to_string().contains("unknown claim"),
        "error must name the missing claim: {err}"
    );
}

#[test]
fn explain_claim_malformed_id_is_parse_error() {
    // A claim id without the required `claim_` prefix must fail ClaimId parsing
    // before any lookup, surfacing a TexoError rather than panicking.
    let dir = setup_workspace();
    let ctx = ToolContext {
        root: dir.path().to_path_buf(),
        workspace_id: None,
    };
    let err = ctx
        .explain_claim(&ExplainClaimInput {
            claim_id: "not-a-claim".to_string(),
        })
        .expect_err("malformed id must error");
    // Non-empty, typed error message proves we did not silently succeed.
    assert!(!err.to_string().is_empty());
}

#[test]
fn check_staleness_missing_workspace_errors() {
    // Opening a tool against a directory that was never `init`ed must error
    // (no .texo config), covering the open/replay failure path shared by all
    // tools rather than the success path only.
    let dir = tempfile::tempdir().expect("tempdir");
    let ctx = ToolContext {
        root: dir.path().to_path_buf(),
        workspace_id: None,
    };
    let result = ctx.check_staleness(&CheckStalenessInput {
        path: "anything.md".to_string(),
    });
    assert!(
        result.is_err(),
        "tools must error when the workspace is not initialized"
    );
}

#[test]
fn mcp_tool_error_converts_to_error_data_and_call_result() {
    // PROVES: the error.rs boundary conversions. A real TexoError (produced by
    // explaining an unknown claim) becomes an McpToolError, then an ErrorData
    // (internal_error code, message preserved), then a CallToolResult flagged
    // as an error. Without this test error.rs sits at 0%.
    use rmcp::handler::server::tool::IntoCallToolResult;
    use rmcp::model::ErrorData;

    let dir = setup_workspace();
    let ctx = ToolContext {
        root: dir.path().to_path_buf(),
        workspace_id: None,
    };
    let texo_err = ctx
        .explain_claim(&ExplainClaimInput {
            claim_id: "claim_ffffffffffff".to_string(),
        })
        .expect_err("unknown claim must error");

    // TexoError -> McpToolError (the `#[from] TexoError` arm).
    let tool_err: McpToolError = texo_err.into();
    let rendered = tool_err.to_string();
    assert!(
        rendered.contains("unknown claim"),
        "McpToolError must preserve the underlying message: {rendered}"
    );

    // McpToolError -> ErrorData (From impl): message preserved, internal code.
    let data: ErrorData = ErrorData::from(McpToolError::Tool(
        ctx.explain_claim(&ExplainClaimInput {
            claim_id: "claim_ffffffffffff".to_string(),
        })
        .expect_err("err"),
    ));
    assert!(
        data.message.contains("unknown claim"),
        "ErrorData must carry the failure message: {}",
        data.message
    );
    assert_eq!(
        data.code,
        rmcp::model::ErrorCode::INTERNAL_ERROR,
        "tool failures must map to the MCP internal-error code"
    );

    // McpToolError -> IntoCallToolResult: a tool failure surfaces as a
    // protocol-level Err(ErrorData) (not an Ok result), preserving the message.
    let call_result = tool_err.into_call_tool_result();
    let wire_err = call_result.expect_err("a tool failure must yield Err(ErrorData)");
    assert!(
        wire_err.message.contains("unknown claim"),
        "the wire error must preserve the failure message: {}",
        wire_err.message
    );
}
