//! MCP stdio server entrypoint.

use std::path::PathBuf;

use rmcp::{
    handler::server::wrapper::Parameters,
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ServerHandler, ServiceExt,
};
use tracing_subscriber::{self, EnvFilter};

use crate::tools::{
    CheckStalenessInput, ExplainClaimInput, GetAgentContextInput, GetCurrentClaimsInput,
    ToolContext,
};

/// texo MCP server handler.
#[derive(Clone)]
pub struct TexoMcpServer {
    ctx: ToolContext,
    // justify: rmcp `tool_router` macro reads this field via generated dispatch code
    #[allow(dead_code)]
    tool_router: rmcp::handler::server::router::tool::ToolRouter<Self>,
}

#[tool_router]
impl TexoMcpServer {
    /// Create server for workspace root.
    pub fn new(root: PathBuf, workspace_id: Option<String>) -> Self {
        Self {
            ctx: ToolContext { root, workspace_id },
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Check whether a markdown document contains claims that are stale, superseded, or contradicted by the local texo claim-chain. Use this before trusting project docs, onboarding notes, architecture notes, process docs, or AI-generated summaries. Returns diagnostics with source lines, superseding claims, receipts, and the local replay frontier. This tool is read-only."
    )]
    async fn check_staleness(
        &self,
        Parameters(input): Parameters<CheckStalenessInput>,
    ) -> Result<String, String> {
        let ctx = self.ctx.clone();
        tokio::task::spawn_blocking(move || ctx.check_staleness(&input).map_err(|e| e.to_string()))
            .await
            .map_err(|e| e.to_string())?
    }

    #[tool(
        description = "Return current non-superseded claims from the local texo claim-chain, optionally filtered by subject. Use this instead of reading raw prose when answering questions about team process, product direction, ownership, architecture, or decisions. Includes provenance, receipts, and local replay frontier. This tool is read-only."
    )]
    async fn get_current_claims(
        &self,
        Parameters(input): Parameters<GetCurrentClaimsInput>,
    ) -> Result<String, String> {
        let ctx = self.ctx.clone();
        tokio::task::spawn_blocking(move || {
            ctx.get_current_claims(&input).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    #[tool(
        description = "Return the structured context snapshot an agent should use for this workspace: current claims, stale claims, conflicts, provenance, receipts, and local replay frontier. Use this when preparing to answer questions from project knowledge or before generating onboarding, architecture, or process summaries. This tool is read-only."
    )]
    async fn get_agent_context(
        &self,
        Parameters(input): Parameters<GetAgentContextInput>,
    ) -> Result<String, String> {
        let ctx = self.ctx.clone();
        tokio::task::spawn_blocking(move || {
            ctx.get_agent_context(&input).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    #[tool(
        description = "Explain one claim from the texo claim-chain by returning its text, source, receipt, supersession trail, conflicts, and local replay frontier. Use this when you need to justify why a claim is current or stale. This tool is read-only."
    )]
    async fn explain_claim(
        &self,
        Parameters(input): Parameters<ExplainClaimInput>,
    ) -> Result<String, String> {
        let ctx = self.ctx.clone();
        tokio::task::spawn_blocking(move || ctx.explain_claim(&input).map_err(|e| e.to_string()))
            .await
            .map_err(|e| e.to_string())?
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

    #[test]
    fn tool_router_builds() {
        let _server = TexoMcpServer::new(std::env::current_dir().expect("cwd"), None);
    }
}
