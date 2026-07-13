//! Sync line-delimited MCP JSON-RPC stdio surface.

use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{json, Value};

use crate::config::TexoRootConfig;
use crate::error::TexoError;
use crate::host::TexoHost;

const LINE_CAP: usize = 1024 * 1024;
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 2] = ["2025-06-18", "2024-11-05"];

/// Run the MCP stdio server over locked standard streams.
///
/// # Errors
///
/// Returns [`TexoError::Io`] when stdin/stdout I/O fails and
/// [`TexoError::Json`] when a response cannot be serialized.
pub fn run(root: &Path, workspace: Option<&str>, journal: Option<&str>) -> Result<(), TexoError> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    run_with_io(stdin.lock(), stdout.lock(), root, workspace, journal)
}

fn run_with_io<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    root: &Path,
    workspace: Option<&str>,
    journal: Option<&str>,
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
            handle_line(&line, root, workspace, journal)
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut output, &response)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

fn handle_line(
    line: &str,
    root: &Path,
    workspace: Option<&str>,
    journal: Option<&str>,
) -> Option<Value> {
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
        "tools/call" => Some(call_tool(
            &id_value,
            value.get("params"),
            root,
            workspace,
            journal,
        )),
        _ => Some(error_response(&id_value, -32601, "method not found", None)),
    }
}

fn initialize_result(value: &Value) -> Value {
    let requested = value
        .get("params")
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);
    let protocol = SUPPORTED_PROTOCOL_VERSIONS
        .into_iter()
        .find(|supported| *supported == requested)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);
    json!({
        "protocolVersion": protocol,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "texo",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": crate::agent_catalog::INSTRUCTIONS
    })
}

fn tools_list() -> Value {
    crate::agent_catalog::mcp_tools_list()
}

fn call_tool(
    id: &Value,
    params: Option<&Value>,
    root: &Path,
    workspace: Option<&str>,
    journal: Option<&str>,
) -> Value {
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
    let mut mapped = match map_tool_input(name, &args) {
        Ok(mapped) => mapped,
        Err(message) => return error_response(id, -32602, &message, None),
    };
    let mut host = match open_host(root, workspace, journal) {
        Ok(host) => host,
        Err(error) => {
            return error_response(id, -32603, &error.to_string(), Some(failure_data(&error)));
        }
    };
    let requested_snapshot = args.get("snapshot_token").cloned().unwrap_or(Value::Null);
    let status = match host.invoke_json(
        "texo.workspace.status",
        &json!({ "snapshot": requested_snapshot }),
    ) {
        Ok(status) => status,
        Err(error) => {
            return error_response(id, -32603, &error.to_string(), Some(failure_data(&error)));
        }
    };
    let Some(snapshot_token) = status
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("token"))
        .and_then(Value::as_str)
    else {
        return error_response(id, -32603, "workspace status omitted snapshot token", None);
    };
    set_snapshot_token(&mut mapped.input, snapshot_token);
    let operation_result = if name == "get_workspace_status" {
        Ok(status.clone())
    } else {
        host.invoke_json(mapped.op, &mapped.input)
    };
    match operation_result {
        Ok(output) => {
            let Some(spec) = crate::agent_catalog::find(name) else {
                return error_response(id, -32602, "unknown tool", None);
            };
            success_response(
                id,
                &json!({
                    "content": [{ "type": "text", "text": tool_summary(name, &output) }],
                    "structuredContent": {
                        "schema": spec.result_schema,
                        "data": output,
                        "meta": status,
                        "next_actions": next_actions(name, &output)
                    },
                    "isError": false
                }),
            )
        }
        Err(error) => error_response(id, -32603, &error.to_string(), Some(failure_data(&error))),
    }
}

fn set_snapshot_token(input: &mut Value, snapshot_token: &str) {
    if let Some(fields) = input.as_object_mut() {
        fields.insert("snapshot".to_string(), Value::from(snapshot_token));
    }
}

fn failure_data(error: &TexoError) -> Value {
    let facts = error.facts();
    json!({
        "code": error.code(),
        "committed": facts.committed,
        "retry_safe": facts.retry_safe,
        "resume": facts.resume,
    })
}

struct MappedTool {
    op: &'static str,
    input: Value,
}

fn map_tool_input(name: &str, args: &Value) -> Result<MappedTool, String> {
    match name {
        "triangulate" => {
            let target = args
                .get("target")
                .cloned()
                .ok_or_else(|| "missing target".to_string())?;
            Ok(MappedTool {
                op: "texo.knowledge.triangulate",
                input: json!({ "target": target }),
            })
        }
        "search_knowledge" => Ok(MappedTool {
            op: "texo.knowledge.search",
            input: json!({
                "query": args.get("query").cloned().unwrap_or(Value::Null),
                "subject": args.get("subject_hint").cloned().unwrap_or(Value::Null),
                "status": args.get("status").cloned().unwrap_or(Value::Null),
                "limit": args.get("limit").cloned().unwrap_or(json!(25)),
                "cursor": args.get("cursor").cloned().unwrap_or(Value::Null)
            }),
        }),
        "get_agent_context" => Ok(MappedTool {
            op: "texo.context.agent",
            input: json!({
                "subject": args.get("subject_hint").cloned().unwrap_or(Value::Null),
                "include_stale": args.get("include_stale").and_then(Value::as_bool).unwrap_or(false)
            }),
        }),
        "explain_knowledge" => {
            let claim_id = args
                .get("claim_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing claim_id".to_string())?;
            Ok(MappedTool {
                op: "texo.claim.explain",
                input: json!({ "claim_id": claim_id }),
            })
        }
        "get_workspace_status" => Ok(MappedTool {
            op: "texo.workspace.status",
            input: json!({
                "snapshot": args.get("snapshot_token").cloned().unwrap_or(Value::Null)
            }),
        }),
        _ => Err("unknown tool".to_string()),
    }
}

fn tool_summary(name: &str, output: &Value) -> String {
    match name {
        "get_agent_context" => format!(
            "Loaded {} current claims and {} open conflicts through frontier {}.",
            array_len(output, "claims"),
            array_len(output, "conflicts"),
            output
                .get("replayed_through_sequence")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        ),
        "search_knowledge" => format!(
            "Returned {} of {} matching claims{}.",
            output.get("returned").and_then(Value::as_u64).unwrap_or(0),
            output.get("total").and_then(Value::as_u64).unwrap_or(0),
            if output.get("has_more").and_then(Value::as_bool) == Some(true) {
                "; more results are available"
            } else {
                ""
            }
        ),
        "explain_knowledge" => {
            "Loaded the claim card and snapshot-bounded journal timeline.".to_string()
        }
        "triangulate" => format!(
            "Triangulation state is {}; found {} assertions, {} exact claim-evidence occurrences, and {} code occurrences.",
            output
                .get("answer_state")
                .and_then(Value::as_str)
                .unwrap_or("unverified"),
            array_len(output, "assertions"),
            array_len(output, "evidence"),
            array_len(output, "structural_evidence")
        ),
        "get_workspace_status" => format!(
            "Workspace is at frontier {}; settlement complete: {}.",
            output.get("frontier").and_then(Value::as_u64).unwrap_or(0),
            output
                .get("settlement_complete")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        ),
        _ => "Texo operation completed.".to_string(),
    }
}

fn array_len(value: &Value, key: &str) -> usize {
    value.get(key).and_then(Value::as_array).map_or(0, Vec::len)
}

fn next_actions(name: &str, output: &Value) -> Value {
    match name {
        "get_agent_context" => json!([{
            "tool": "search_knowledge",
            "reason": "Narrow the workspace context to the task when needed.",
            "arguments": { "snapshot_token": output_snapshot_token(output) }
        }]),
        "search_knowledge" => {
            let first = output
                .get("results")
                .and_then(Value::as_array)
                .and_then(|results| results.first());
            match first.and_then(|result| {
                result
                    .get("kind")
                    .and_then(Value::as_str)
                    .map(|kind| (kind, result))
            }) {
                Some(("claim", result)) => {
                    let claim_id = result
                        .get("claim")
                        .and_then(|claim| claim.get("claim_id"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    json!([{
                        "tool": "explain_knowledge",
                        "reason": "Inspect provenance and authority for a matching claim.",
                        "arguments": {
                            "claim_id": claim_id,
                            "snapshot_token": output_snapshot_token(output)
                        }
                    }])
                }
                Some(("code", result)) => {
                    let symbol = result
                        .get("occurrence")
                        .and_then(|occurrence| occurrence.get("symbol"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    json!([{
                        "tool": "triangulate",
                        "reason": "Inspect exact code occurrences and coverage for the matching symbol.",
                        "arguments": {
                            "target": { "kind": "symbol", "symbol": symbol },
                            "snapshot_token": output_snapshot_token(output)
                        }
                    }])
                }
                _ => json!([]),
            }
        }
        "triangulate" => json!([{
            "tool": "get_agent_context",
            "reason": "Load current and replacement claims when triangulation is stale, contradicted, or incomplete.",
            "arguments": {
                "include_stale": true,
                "snapshot_token": output_snapshot_token(output)
            }
        }]),
        _ => json!([]),
    }
}

fn output_snapshot_token(output: &Value) -> Value {
    output
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("token"))
        .cloned()
        .unwrap_or(Value::Null)
}

fn open_host(
    root: &Path,
    workspace: Option<&str>,
    journal: Option<&str>,
) -> Result<TexoHost, TexoError> {
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
    if let Some(journal) = journal {
        TexoHost::open_journal(
            root.to_path_buf(),
            workspace,
            journal,
            crate::surfaces::cli::observed_at_ms(),
        )
    } else {
        TexoHost::open(
            root.to_path_buf(),
            workspace,
            crate::surfaces::cli::observed_at_ms(),
        )
    }
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
    fn initialize_negotiates_supported_protocol_and_server_name_is_compactable() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "test-version" }
        });
        let response =
            handle_line(&request.to_string(), Path::new("."), None, None).expect("response");
        assert_eq!(
            response["result"]["protocolVersion"],
            DEFAULT_PROTOCOL_VERSION
        );
        let compact = serde_json::to_string(&response).expect("serialize response");
        assert!(compact.contains("\"name\":\"texo\""));
    }

    #[test]
    fn notification_has_no_response() {
        let request = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        assert!(handle_line(&request.to_string(), Path::new("."), None, None).is_none());
    }

    #[test]
    fn tools_list_has_five_tools() {
        let tools = tools_list();
        assert_eq!(tools["tools"].as_array().expect("tools array").len(), 5);
    }

    #[test]
    fn code_search_result_routes_to_snapshot_bound_symbol_triangulation() {
        let output = json!({
            "results": [{
                "kind": "code",
                "occurrence": { "symbol": "rust-analyzer cargo demo 0.1.0 lib/deploy()." }
            }],
            "snapshot": { "token": "texo_snap_v1.example" }
        });
        let actions = next_actions("search_knowledge", &output);
        assert_eq!(actions[0]["tool"], "triangulate");
        assert_eq!(actions[0]["arguments"]["target"]["kind"], "symbol");
        assert_eq!(
            actions[0]["arguments"]["target"]["symbol"],
            "rust-analyzer cargo demo 0.1.0 lib/deploy()."
        );
        assert_eq!(
            actions[0]["arguments"]["snapshot_token"],
            "texo_snap_v1.example"
        );
    }
}
