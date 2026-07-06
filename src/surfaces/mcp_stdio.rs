//! Sync line-delimited MCP JSON-RPC stdio surface.

use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{json, Value};

use crate::config::TexoRootConfig;
use crate::error::TexoError;
use crate::host::TexoHost;

const LINE_CAP: usize = 1024 * 1024;
const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";

/// Run the MCP stdio server over locked standard streams.
///
/// # Errors
///
/// Returns [`TexoError::Io`] when stdin/stdout I/O fails and
/// [`TexoError::Json`] when a response cannot be serialized.
pub fn run(root: &Path, workspace: Option<&str>) -> Result<(), TexoError> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    run_with_io(stdin.lock(), stdout.lock(), root, workspace)
}

fn run_with_io<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    root: &Path,
    workspace: Option<&str>,
) -> Result<(), TexoError> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = input.read_line(&mut line)?;
        if read == 0 {
            return Ok(());
        }
        let response = if line.len() > LINE_CAP {
            Some(error_response(&Value::Null, -32700, "parse error", None))
        } else {
            handle_line(&line, root, workspace)
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut output, &response)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

fn handle_line(line: &str, root: &Path, workspace: Option<&str>) -> Option<Value> {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Some(error_response(&Value::Null, -32700, "parse error", None));
    };
    let id = value.get("id").cloned();
    let method = value.get("method").and_then(Value::as_str);
    if id.is_none() && method.is_some_and(|method| method.starts_with("notifications/")) {
        return None;
    }
    let id_value = id.unwrap_or(Value::Null);
    let Some(method) = method else {
        return Some(error_response(&id_value, -32600, "invalid request", None));
    };
    match method {
        "initialize" => Some(success_response(&id_value, &initialize_result(&value))),
        "tools/list" => Some(success_response(&id_value, &tools_list())),
        "tools/call" => Some(call_tool(&id_value, value.get("params"), root, workspace)),
        _ => Some(error_response(&id_value, -32601, "method not found", None)),
    }
}

fn initialize_result(value: &Value) -> Value {
    let protocol = value
        .get("params")
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);
    json!({
        "protocolVersion": protocol,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "texo",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": "Read-only access to the local texo claim-chain. Prefer these tools over stale markdown."
    })
}

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "check_staleness",
                "description": "Check whether a markdown document contains claims that are stale, superseded, or contradicted by the local texo claim-chain. Use this before trusting project docs, onboarding notes, architecture notes, process docs, or AI-generated summaries. Returns diagnostics with source lines, superseding claims, receipts, and the local replay frontier. This tool is read-only.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"],
                    "additionalProperties": false
                }
            },
            {
                "name": "get_current_claims",
                "description": "Return current non-superseded claims from the local texo claim-chain, optionally filtered by subject. Use this instead of reading raw prose when answering questions about team process, product direction, ownership, architecture, or decisions. Includes provenance, receipts, and local replay frontier. This tool is read-only.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "subject_hint": { "type": ["string", "null"] } },
                    "additionalProperties": false
                }
            },
            {
                "name": "get_agent_context",
                "description": "Return the structured context snapshot an agent should use for this workspace: current claims, stale claims, conflicts, provenance, receipts, and local replay frontier. Use this when preparing to answer questions from project knowledge or before generating onboarding, architecture, or process summaries. This tool is read-only.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "subject_hint": { "type": ["string", "null"] },
                        "include_stale": { "type": "boolean", "default": false }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "explain_claim",
                "description": "Explain one claim from the texo claim-chain by returning its text, source, receipt, supersession trail, conflicts, and local replay frontier. Use this when you need to justify why a claim is current or stale. This tool is read-only.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "claim_id": { "type": "string" } },
                    "required": ["claim_id"],
                    "additionalProperties": false
                }
            }
        ]
    })
}

fn call_tool(id: &Value, params: Option<&Value>, root: &Path, workspace: Option<&str>) -> Value {
    let Some(params) = params else {
        return error_response(id, -32602, "invalid params", None);
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return error_response(id, -32602, "invalid params", None);
    };
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mapped = match map_tool_input(name, &args) {
        Ok(mapped) => mapped,
        Err(message) => return error_response(id, -32602, &message, None),
    };
    let mut host = match open_host(root, workspace) {
        Ok(host) => host,
        Err(error) => {
            return error_response(
                id,
                -32603,
                &error.to_string(),
                Some(json!({ "code": error.code() })),
            );
        }
    };
    match host.invoke_json(mapped.op, &mapped.input) {
        Ok(output) => {
            let text = match serde_json::to_string_pretty(&output) {
                Ok(text) => text,
                Err(error) => {
                    return error_response(
                        id,
                        -32603,
                        &error.to_string(),
                        Some(json!({"code": "json"})),
                    );
                }
            };
            success_response(
                id,
                &json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                }),
            )
        }
        Err(error) => error_response(
            id,
            -32603,
            &error.to_string(),
            Some(json!({ "code": error.code() })),
        ),
    }
}

struct MappedTool {
    op: &'static str,
    input: Value,
}

fn map_tool_input(name: &str, args: &Value) -> Result<MappedTool, String> {
    match name {
        "check_staleness" => {
            let path = args
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing path".to_string())?;
            Ok(MappedTool {
                op: "texo.staleness.check",
                input: json!({ "path": path }),
            })
        }
        "get_current_claims" => Ok(MappedTool {
            op: "texo.claims.list",
            input: json!({ "subject": args.get("subject_hint").cloned().unwrap_or(Value::Null) }),
        }),
        "get_agent_context" => Ok(MappedTool {
            op: "texo.context.agent",
            input: json!({
                "subject": args.get("subject_hint").cloned().unwrap_or(Value::Null),
                "include_stale": args.get("include_stale").and_then(Value::as_bool).unwrap_or(false)
            }),
        }),
        "explain_claim" => {
            let claim_id = args
                .get("claim_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing claim_id".to_string())?;
            Ok(MappedTool {
                op: "texo.claim.explain",
                input: json!({ "claim_id": claim_id }),
            })
        }
        _ => Err("unknown tool".to_string()),
    }
}

fn open_host(root: &Path, workspace: Option<&str>) -> Result<TexoHost, TexoError> {
    let workspace = if let Some(workspace) = workspace {
        workspace.to_string()
    } else {
        let config_path = root.join(".texo").join("config.toml");
        if config_path.exists() {
            TexoRootConfig::load(&config_path)
                .map_err(|error| TexoError::Config {
                    detail: error.to_string(),
                    source: Some(Box::new(error)),
                })?
                .resolve(None)
                .map(|config| config.workspace_id)
                .map_err(|error| TexoError::Config {
                    detail: error.to_string(),
                    source: Some(Box::new(error)),
                })?
        } else {
            "demo".to_string()
        }
    };
    TexoHost::open(
        root.to_path_buf(),
        workspace,
        crate::surfaces::cli::observed_at_ms(),
    )
}

fn success_response(id: &Value, result: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: &Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_echoes_protocol_and_server_name_is_compactable() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "test-version" }
        });
        let response = handle_line(&request.to_string(), Path::new("."), None).expect("response");
        assert_eq!(response["result"]["protocolVersion"], "test-version");
        let compact = serde_json::to_string(&response).expect("serialize response");
        assert!(compact.contains("\"name\":\"texo\""));
    }

    #[test]
    fn notification_has_no_response() {
        let request = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        assert!(handle_line(&request.to_string(), Path::new("."), None).is_none());
    }

    #[test]
    fn tools_list_has_four_tools() {
        let tools = tools_list();
        assert_eq!(tools["tools"].as_array().expect("tools array").len(), 4);
    }
}
