//! Curated agent-facing catalog shared by MCP, install guidance, and ops UX.

use serde::Serialize;
use serde_json::{json, Value};

/// Stable schema for the curated agent catalog.
pub const CATALOG_SCHEMA: &str = "texo.agent-catalog.v1";

/// One curated read-only agent tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AgentToolSpec {
    /// MCP-visible tool name.
    pub name: &'static str,
    /// Existing typed Texo operation invoked by the tool.
    pub operation: &'static str,
    /// Human- and model-facing usage guidance.
    pub description: &'static str,
    /// Schema identifier for structured successful output.
    pub result_schema: &'static str,
}

/// Return the five-tool progressive-disclosure catalog.
#[must_use]
pub fn tools() -> Vec<AgentToolSpec> {
    vec![
        AgentToolSpec {
            name: "get_agent_context",
            operation: "texo.context.agent",
            description: "Start here when answering from workspace knowledge. Returns current claims, conflicts, freshness, provenance, and settlement warnings. Search claims next when the snapshot is broader than the task. This tool is read-only.",
            result_schema: "texo.mcp.agent-context.v1",
        },
        AgentToolSpec {
            name: "search_claims",
            operation: "texo.claims.search",
            description: "Search the local claim-chain lexically with bounded pagination and optional subject/status filters. Use explain_claim on a returned claim id when provenance or authority matters. This tool is read-only.",
            result_schema: "texo.mcp.claim-search.v1",
        },
        AgentToolSpec {
            name: "explain_claim",
            operation: "texo.claim.explain",
            description: "Explain one claim's source, receipt, timeline, supersession trail, and conflicts. Use this after get_agent_context or search_claims when you need to justify why a claim is current or stale. This tool is read-only.",
            result_schema: "texo.mcp.claim-explain.v1",
        },
        AgentToolSpec {
            name: "check_staleness",
            operation: "texo.staleness.check",
            description: "Check a markdown file before trusting, editing, or summarizing it. Returns line-level stale/superseded diagnostics and evidence. This tool is read-only.",
            result_schema: "texo.mcp.staleness.v1",
        },
        AgentToolSpec {
            name: "get_workspace_status",
            operation: "texo.workspace.status",
            description: "Inspect projection freshness, frontier, claim/conflict counts, and semantic settlement completeness. Use this when another tool reports incomplete or stale evidence. This tool is read-only.",
            result_schema: "texo.mcp.workspace-status.v1",
        },
    ]
}

/// Find one curated tool by MCP-visible name.
#[must_use]
pub fn find(name: &str) -> Option<AgentToolSpec> {
    tools().into_iter().find(|tool| tool.name == name)
}

/// Build the MCP `tools/list` payload from the shared catalog.
#[must_use]
pub fn mcp_tools_list() -> Value {
    let tools = tools()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": input_schema(tool.name),
                "annotations": {
                    "readOnlyHint": true,
                    "destructiveHint": false,
                    "idempotentHint": true,
                    "openWorldHint": false
                }
            })
        })
        .collect::<Vec<_>>();
    json!({ "tools": tools })
}

/// Render the complete operation inventory for developer discovery.
#[must_use]
pub fn operation_inventory() -> Value {
    let exposed = tools();
    let mut operations = crate::ops::catalog()
        .into_iter()
        .map(|item| {
            let descriptor = item.descriptor();
            let agent_tool = exposed
                .iter()
                .find(|tool| tool.operation == descriptor.name())
                .map(|tool| tool.name);
            json!({
                "name": descriptor.name(),
                "effect": descriptor.effect.as_str(),
                "input_schema": descriptor.input_schema_ref(),
                "output_schema": descriptor.output_schema_ref(),
                "receipt_kind": descriptor.receipt_kind(),
                "agent_tool": agent_tool,
                "mcp": agent_tool.is_some()
            })
        })
        .collect::<Vec<_>>();
    operations.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    json!({
        "schema": CATALOG_SCHEMA,
        "operations": operations
    })
}

/// Input schema for one curated tool.
#[must_use]
pub fn input_schema(name: &str) -> Value {
    match name {
        "get_agent_context" => json!({
            "type": "object",
            "properties": {
                "subject_hint": { "type": ["string", "null"] },
                "include_stale": { "type": "boolean", "default": false }
            },
            "additionalProperties": false
        }),
        "search_claims" => json!({
            "type": "object",
            "properties": {
                "query": { "type": ["string", "null"], "maxLength": 256 },
                "subject_hint": { "type": ["string", "null"] },
                "status": {
                    "type": ["string", "null"],
                    "enum": ["current", "superseded", "conflicting", null]
                },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 25 },
                "cursor": { "type": ["string", "null"] }
            },
            "additionalProperties": false
        }),
        "explain_claim" => json!({
            "type": "object",
            "properties": { "claim_id": { "type": "string" } },
            "required": ["claim_id"],
            "additionalProperties": false
        }),
        "check_staleness" => json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
            "additionalProperties": false
        }),
        _ => json!({
            "type": "object",
            "additionalProperties": false
        }),
    }
}

/// Recommended agent workflow emitted during MCP initialization and install.
pub const INSTRUCTIONS: &str = "Start with get_agent_context before answering from project knowledge. Use search_claims for bounded discovery, explain_claim for provenance, check_staleness before trusting or editing documentation, and get_workspace_status when evidence is stale or incomplete. All tools are local and read-only; absence of a verdict never means unrelated.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_five_unique_read_only_tools() {
        let tools = tools();
        assert_eq!(tools.len(), 5);
        let names = tools
            .iter()
            .map(|tool| tool.name)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names.len(), tools.len());
        let listed = mcp_tools_list();
        for tool in listed["tools"].as_array().expect("tools array") {
            assert_eq!(tool["annotations"]["readOnlyHint"], true);
        }
    }

    #[test]
    fn every_agent_tool_routes_to_a_registered_operation() {
        let operations = crate::ops::catalog()
            .into_iter()
            .map(|item| item.descriptor().name().to_string())
            .collect::<std::collections::BTreeSet<_>>();
        for tool in tools() {
            assert!(
                operations.contains(tool.operation),
                "missing operation {}",
                tool.operation
            );
        }
    }
}
