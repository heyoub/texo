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
        assert_eq!(
            names,
            vec![
                "check_staleness".to_string(),
                "explain_claim".to_string(),
                "get_agent_context".to_string(),
                "get_current_claims".to_string(),
            ],
            "tool_router must register exactly the four read-only tools"
        );
        for tool in [
            "check_staleness",
            "explain_claim",
            "get_agent_context",
            "get_current_claims",
        ] {
            assert!(
                router.has_route(tool),
                "router must have a route for `{tool}`"
            );
        }
    }

    /// `get_info` must advertise the texo server identity and enable the tools
    /// capability so MCP clients discover the read-only tool surface.
    #[test]
    fn get_info_advertises_tools_capability() {
        let server = TexoMcpServer::new(std::env::current_dir().expect("cwd"), None);
        let info = server.get_info();
        assert_eq!(info.server_info.name, "texo");
        assert!(
            info.capabilities.tools.is_some(),
            "server must advertise the tools capability"
        );
        assert!(
            info.instructions
                .as_deref()
                .is_some_and(|i| i.contains("texo claim-chain")),
            "server instructions must steer agents to the claim-chain"
        );
    }
}
