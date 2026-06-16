//! MCP stdio server entrypoint.

use std::path::PathBuf;

use rmcp::{
    handler::server::wrapper::Parameters,
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ServerHandler, ServiceExt,
};
use tracing_subscriber::{self, EnvFilter};

use crate::error::McpToolError;
use crate::tools::{
    CheckStalenessInput, ExplainClaimInput, GetAgentContextInput, GetCurrentClaimsInput,
    ToolContext,
};

/// texo MCP server handler.
///
/// No `tool_router` field is stored: the `#[tool_router]` macro emits a
/// `Self::tool_router()` associated function, and `#[tool_handler]` dispatches
/// through that function (its default `router` expression) rather than through a
/// struct field.
#[derive(Clone)]
pub struct TexoMcpServer {
    ctx: ToolContext,
}

#[tool_router]
impl TexoMcpServer {
    /// Create server for workspace root.
    pub fn new(root: PathBuf, workspace_id: Option<String>) -> Self {
        Self {
            ctx: ToolContext { root, workspace_id },
        }
    }

    #[tool(
        description = "Check whether a markdown document contains claims that are stale, superseded, or contradicted by the local texo claim-chain. Use this before trusting project docs, onboarding notes, architecture notes, process docs, or AI-generated summaries. Returns diagnostics with source lines, superseding claims, receipts, and the local replay frontier. This tool is read-only."
    )]
    async fn check_staleness(
        &self,
        Parameters(input): Parameters<CheckStalenessInput>,
    ) -> Result<String, McpToolError> {
        let ctx = self.ctx.clone();
        let output = tokio::task::spawn_blocking(move || ctx.check_staleness(&input)).await??;
        Ok(output)
    }

    #[tool(
        description = "Return current non-superseded claims from the local texo claim-chain, optionally filtered by subject. Use this instead of reading raw prose when answering questions about team process, product direction, ownership, architecture, or decisions. Includes provenance, receipts, and local replay frontier. This tool is read-only."
    )]
    async fn get_current_claims(
        &self,
        Parameters(input): Parameters<GetCurrentClaimsInput>,
    ) -> Result<String, McpToolError> {
        let ctx = self.ctx.clone();
        let output = tokio::task::spawn_blocking(move || ctx.get_current_claims(&input)).await??;
        Ok(output)
    }

    #[tool(
        description = "Return the structured context snapshot an agent should use for this workspace: current claims, stale claims, conflicts, provenance, receipts, and local replay frontier. Use this when preparing to answer questions from project knowledge or before generating onboarding, architecture, or process summaries. This tool is read-only."
    )]
    async fn get_agent_context(
        &self,
        Parameters(input): Parameters<GetAgentContextInput>,
    ) -> Result<String, McpToolError> {
        let ctx = self.ctx.clone();
        let output = tokio::task::spawn_blocking(move || ctx.get_agent_context(&input)).await??;
        Ok(output)
    }

    #[tool(
        description = "Explain one claim from the texo claim-chain by returning its text, source, receipt, supersession trail, conflicts, and local replay frontier. Use this when you need to justify why a claim is current or stale. This tool is read-only."
    )]
    async fn explain_claim(
        &self,
        Parameters(input): Parameters<ExplainClaimInput>,
    ) -> Result<String, McpToolError> {
        let ctx = self.ctx.clone();
        let output = tokio::task::spawn_blocking(move || ctx.explain_claim(&input)).await??;
        Ok(output)
    }
}

#[tool_handler]
impl ServerHandler for TexoMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "texo",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Read-only access to the local texo claim-chain. Prefer these tools over stale markdown.",
            )
    }
}

/// Run MCP server over stdio. Logs go to stderr.
pub async fn run_stdio(root: PathBuf, workspace_id: Option<String>) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("texo_mcp=info".parse()?))
        .with_writer(std::io::stderr)
        .init();

    let server = TexoMcpServer::new(root, workspace_id);
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use texo_core::{
        ingest_sources, init_workspace, open_journal, IngestMode, FIXTURE_OBSERVED_AT_MS,
    };

    /// Build a real, ingested workspace in a tempdir so the async `#[tool]`
    /// wrappers run against genuine journal/replay state (no test doubles).
    fn ingested_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        init_workspace(dir.path(), "demo").expect("init");
        let sample_src =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sample_sources");
        let dest = dir.path().join("sample_sources");
        std::fs::create_dir_all(&dest).expect("mkdir sample_sources");
        for entry in std::fs::read_dir(&sample_src).expect("read sample_sources") {
            let entry = entry.expect("entry");
            std::fs::copy(entry.path(), dest.join(entry.file_name())).expect("copy sample");
        }
        let journal = open_journal(dir.path()).expect("open");
        let workspace = journal.config().workspace().expect("workspace");
        ingest_sources(
            journal.handle(),
            journal.config(),
            &workspace,
            &dest,
            IngestMode::Commit,
            FIXTURE_OBSERVED_AT_MS,
            dir.path(),
        )
        .expect("ingest");
        journal.close().expect("close");
        dir
    }

    /// The `#[tool_router]` macro must register every annotated handler. Assert
    /// the router exposes exactly the four read-only tools by name, not merely
    /// that the server constructs (the old, tautological assertion).
    #[test]
    fn tool_router_registers_all_tools() {
        let router = TexoMcpServer::tool_router();
        let mut names: Vec<String> = router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();
        // The router registers exactly the four read-only tools.
        assert_eq!(
            names,
            vec![
                "check_staleness".to_string(),
                "explain_claim".to_string(),
                "get_agent_context".to_string(),
                "get_current_claims".to_string(),
            ]
        );
        for tool in [
            "check_staleness",
            "explain_claim",
            "get_agent_context",
            "get_current_claims",
        ] {
            assert!(router.has_route(tool));
        }
    }

    /// `get_info` must advertise the texo server identity and enable the tools
    /// capability so MCP clients discover the read-only tool surface.
    #[test]
    fn get_info_advertises_tools_capability() {
        let server = TexoMcpServer::new(std::env::current_dir().expect("cwd"), None);
        let info = server.get_info();
        assert_eq!(info.server_info.name, "texo");
        // The server advertises the tools capability and steers agents to the chain.
        assert!(info.capabilities.tools.is_some());
        assert!(info
            .instructions
            .as_deref()
            .is_some_and(|i| i.contains("texo claim-chain")));
    }

    /// PROVES the async `#[tool]` wrapper bodies (`spawn_blocking` + `await??`),
    /// which the synchronous integration tests never enter because they call the
    /// inner `ctx.*` methods directly. Awaiting every wrapper against a real
    /// ingested store exercises lines 44-83 and asserts the JSON each returns.
    #[tokio::test]
    async fn async_tool_wrappers_return_real_content() {
        let dir = ingested_workspace();
        let server = TexoMcpServer::new(dir.path().to_path_buf(), None);

        // get_agent_context: full snapshot with claims + frontier.
        let ctx_json = server
            .get_agent_context(Parameters(GetAgentContextInput {
                subject_hint: None,
                include_stale: true,
            }))
            .await
            .expect("agent context wrapper");
        let ctx_val: serde_json::Value = serde_json::from_str(&ctx_json).expect("agent ctx json");
        // The agent-context wrapper carries an advanced frontier and current claims.
        assert!(ctx_val["replayed_through_sequence"].as_u64().unwrap_or(0) > 0);
        assert!(!ctx_val["claims"].as_array().unwrap_or(&vec![]).is_empty());

        // get_current_claims: claims array + frontier.
        let claims_json = server
            .get_current_claims(Parameters(GetCurrentClaimsInput { subject_hint: None }))
            .await
            .expect("current claims wrapper");
        let claims_val: serde_json::Value =
            serde_json::from_str(&claims_json).expect("claims json");
        let claim_id = claims_val["claims"][0]["claim_id"]
            .as_str()
            .expect("a current claim id from the wrapper")
            .to_string();
        // The current-claims wrapper carries a frontier.
        assert!(
            claims_val["replayed_through_sequence"]
                .as_u64()
                .unwrap_or(0)
                > 0
        );

        // explain_claim: provenance for the id discovered above.
        let explain_json = server
            .explain_claim(Parameters(ExplainClaimInput {
                claim_id: claim_id.clone(),
            }))
            .await
            .expect("explain wrapper");
        let explain_val: serde_json::Value =
            serde_json::from_str(&explain_json).expect("explain json");
        // The explain wrapper echoes the requested claim id.
        assert_eq!(explain_val["claim_id"].as_str(), Some(claim_id.as_str()));

        // check_staleness: diagnostics for a known-stale sample doc.
        let stale_json = server
            .check_staleness(Parameters(CheckStalenessInput {
                path: "sample_sources/stale_onboarding.md".to_string(),
            }))
            .await
            .expect("staleness wrapper");
        let stale_val: serde_json::Value =
            serde_json::from_str(&stale_json).expect("staleness json");
        assert!(
            stale_val["diagnostics"]
                .as_array()
                .is_some_and(|d| !d.is_empty()),
            "staleness wrapper must flag the stale onboarding doc: {stale_val}"
        );
    }

    /// PROVES the async wrapper propagates a typed failure (the `await??` error
    /// arm): explaining an unknown-but-well-formed claim id surfaces an
    /// `McpToolError` rather than an `Ok`, with the underlying message intact.
    #[tokio::test]
    async fn async_wrapper_propagates_tool_error() {
        let dir = ingested_workspace();
        let server = TexoMcpServer::new(dir.path().to_path_buf(), None);
        let err = server
            .explain_claim(Parameters(ExplainClaimInput {
                claim_id: "claim_ffffffffffff".to_string(),
            }))
            .await
            .expect_err("unknown claim must error through the async wrapper");
        assert!(
            err.to_string().contains("unknown claim"),
            "wrapper must surface the typed domain error: {err}"
        );
    }
}
