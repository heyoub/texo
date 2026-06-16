//! Error type bridging texo-core failures to the MCP tool boundary.

use rmcp::handler::server::tool::IntoCallToolResult;
use rmcp::model::CallToolResult;
use rmcp::ErrorData;
use texo_core::TexoError;
use tokio::task::JoinError;

/// Failure surfaced while executing an MCP tool.
///
/// Carries the originating typed error (a [`TexoError`] from the tool body or a
/// [`JoinError`] from the blocking task) so [`std::error::Error::source`] yields
/// the real cause instead of a stringified copy. The conversion into an
/// [`ErrorData`] for the wire happens once, at the tool boundary.
#[derive(Debug, thiserror::Error)]
pub enum McpToolError {
    /// The tool body returned a texo-core error.
    #[error(transparent)]
    Tool(#[from] TexoError),
    /// The blocking task running the tool body panicked or was cancelled.
    #[error("tool task failed: {0}")]
    Join(#[from] JoinError),
}

impl From<McpToolError> for ErrorData {
    fn from(error: McpToolError) -> Self {
        ErrorData::internal_error(error.to_string(), None)
    }
}

impl IntoCallToolResult for McpToolError {
    fn into_call_tool_result(self) -> Result<CallToolResult, ErrorData> {
        ErrorData::from(self).into_call_tool_result()
    }
}
