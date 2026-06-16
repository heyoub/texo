//! MCP tool handlers calling texo-core replay paths.

use std::path::Path;

use schemars::JsonSchema;
use serde::Deserialize;
use texo_core::{
    build_agent_context, check_staleness, explain_claim, open_journal_with, ClaimId, TexoError,
};

/// Input for `check_staleness`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CheckStalenessInput {
    /// Path to markdown file or folder.
    pub path: String,
}

/// Input for `get_current_claims`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GetCurrentClaimsInput {
    /// Optional subject filter.
    pub subject_hint: Option<String>,
}

/// Input for `get_agent_context`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GetAgentContextInput {
    /// Optional subject filter.
    pub subject_hint: Option<String>,
    /// Include stale claims in output.
    #[serde(default)]
    pub include_stale: bool,
}

/// Input for `explain_claim`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ExplainClaimInput {
    /// Claim id to explain.
    pub claim_id: String,
}

/// MCP tool execution context.
#[derive(Clone)]
pub struct ToolContext {
    /// Workspace root directory.
    pub root: std::path::PathBuf,
    /// Optional BatPak workspace scope id.
    pub workspace_id: Option<String>,
}

impl ToolContext {
    fn open(&self) -> Result<texo_core::Journal<texo_core::Open>, TexoError> {
        open_journal_with(&self.root, self.workspace_id.as_deref())
    }

    /// Execute check_staleness tool.
    pub fn check_staleness(&self, input: &CheckStalenessInput) -> Result<String, TexoError> {
        let journal = self.open()?;
        let workspace = journal.config().workspace()?;
        let replayed = journal.replay(&workspace)?;
        let path = Path::new(&input.path);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let report = check_staleness(&replayed.state, &workspace, &resolved, &self.root)?;
        journal.close()?;
        Ok(serde_json::to_string_pretty(&report)?)
    }

    /// Execute get_current_claims tool.
    pub fn get_current_claims(&self, input: &GetCurrentClaimsInput) -> Result<String, TexoError> {
        let journal = self.open()?;
        let workspace = journal.config().workspace()?;
        let replayed = journal.replay(&workspace)?;
        let context =
            build_agent_context(&replayed.state, &workspace, input.subject_hint.as_deref());
        journal.close()?;
        let output = serde_json::json!({
            "claims": context.claims,
            "replayed_through_sequence": context.replayed_through_sequence,
        });
        Ok(serde_json::to_string_pretty(&output)?)
    }

    /// Execute get_agent_context tool.
    pub fn get_agent_context(&self, input: &GetAgentContextInput) -> Result<String, TexoError> {
        let journal = self.open()?;
        let workspace = journal.config().workspace()?;
        let replayed = journal.replay(&workspace)?;
        let mut context =
            build_agent_context(&replayed.state, &workspace, input.subject_hint.as_deref());
        if !input.include_stale {
            context.stale_claims.clear();
        }
        journal.close()?;
        Ok(serde_json::to_string_pretty(&context)?)
    }

    /// Execute explain_claim tool.
    pub fn explain_claim(&self, input: &ExplainClaimInput) -> Result<String, TexoError> {
        let journal = self.open()?;
        let workspace = journal.config().workspace()?;
        let replayed = journal.replay(&workspace)?;
        let claim_id = ClaimId::try_from(input.claim_id.as_str())?;
        let explanation = explain_claim(&replayed.state, &claim_id)
            .ok_or_else(|| TexoError::domain(format!("unknown claim {claim_id}")))?;
        journal.close()?;
        Ok(serde_json::to_string_pretty(&explanation)?)
    }
}
