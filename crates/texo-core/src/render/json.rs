//! JSON artifact rendering.

use crate::agent::context::AgentContext;
use crate::replay::state::ClaimState;
use crate::stale::diagnostic::StalenessReport;
use crate::state::conflict_lifecycle::ConflictReport;

/// Write claims.json content.
pub fn render_claims_json(state: &ClaimState) -> serde_json::Result<String> {
    serde_json::to_string_pretty(state)
}

/// Write stale-context.json from a report.
pub fn render_stale_json(report: &StalenessReport) -> serde_json::Result<String> {
    serde_json::to_string_pretty(report)
}

/// Write conflicts.json.
pub fn render_conflicts_json(report: &ConflictReport) -> serde_json::Result<String> {
    serde_json::to_string_pretty(report)
}

/// Write agent-context.json.
pub fn render_agent_json(context: &AgentContext) -> serde_json::Result<String> {
    serde_json::to_string_pretty(context)
}
