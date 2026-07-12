//! MCP stdio wire tests.

mod support;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::Command;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};

use assert_cmd::prelude::*;
use serde_json::{json, Value};
use support::{ingest_courtroom, TestResult, TestWorkspace};

fn spawn_mcp(root: &Path) -> TestResult<(Child, ChildStdin, BufReader<ChildStdout>)> {
    let mut command = Command::cargo_bin("texo")?;
    let mut child = command
        .arg("--root")
        .arg(root)
        .arg("--workspace")
        .arg("demo")
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let stdin = child.stdin.take().expect("child stdin is piped");
    let stdout = child.stdout.take().expect("child stdout is piped");
    Ok((child, stdin, BufReader::new(stdout)))
}

fn send_json(stdin: &mut ChildStdin, value: &Value) -> TestResult {
    serde_json::to_writer(&mut *stdin, value)?;
    stdin.write_all(b"\n")?;
    stdin.flush()?;
    Ok(())
}

fn read_json(stdout: &mut BufReader<ChildStdout>) -> TestResult<Value> {
    let mut line = String::new();
    let read = stdout.read_line(&mut line)?;
    assert!(read > 0, "child should write one JSON-RPC line");
    Ok(serde_json::from_str(&line)?)
}

fn assert_tool_catalog(tools: &Value) -> TestResult {
    let tool_list = tools["result"]["tools"]
        .as_array()
        .ok_or("tools/list returns an array")?;
    assert_eq!(tool_list.len(), 5);
    assert_eq!(tool_list[0]["name"], "get_agent_context");
    assert_eq!(tool_list[1]["name"], "search_knowledge");
    assert_eq!(tool_list[3]["name"], "triangulate");
    assert_eq!(
        tool_list[3]["inputSchema"]["properties"]["target"]["oneOf"]
            .as_array()
            .map(Vec::len),
        Some(3)
    );
    assert_eq!(tool_list[4]["name"], "get_workspace_status");
    assert!(tool_list
        .iter()
        .all(|tool| tool["annotations"]["readOnlyHint"] == true));
    assert!(tool_list
        .iter()
        .all(|tool| tool["outputSchema"]["required"].is_array()));
    Ok(())
}

fn assert_triangulation_call(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    snapshot_token: &str,
) -> TestResult {
    send_json(
        stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "triangulate",
                "arguments": {
                    "target": {
                        "kind": "path",
                        "path": "docs/old.md",
                        "line_start": null,
                        "line_end": null
                    },
                    "snapshot_token": snapshot_token
                }
            }
        }),
    )?;
    let triangulated = read_json(stdout)?;
    let structured = &triangulated["result"]["structuredContent"];
    assert_eq!(structured["schema"], "texo.mcp.triangulation.v2");
    assert_eq!(structured["data"]["snapshot"]["token"], snapshot_token);
    assert!(structured["data"]["answer_state"].is_string());
    assert!(structured["data"]["uncertainty"].is_array());
    Ok(())
}

#[test]
fn mcp_stdio_full_session() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    ingest_courtroom(&mut workspace)?;
    let root = workspace.root().to_path_buf();
    let support::TestWorkspace { dir: _dir, host } = workspace;
    drop(host);
    let (mut child, mut stdin, mut stdout) = spawn_mcp(&root)?;

    send_json(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-06-18" }
        }),
    )?;
    let initialize = read_json(&mut stdout)?;
    assert_eq!(initialize["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(initialize["result"]["serverInfo"]["name"], "texo");

    send_json(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )?;

    send_json(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    )?;
    let tools = read_json(&mut stdout)?;
    assert_tool_catalog(&tools)?;

    send_json(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "search_knowledge",
                "arguments": { "query": "deploy", "limit": 2 }
            }
        }),
    )?;
    let claims = read_json(&mut stdout)?;
    assert!(claims["result"]["content"][0]["text"]
        .as_str()
        .expect("tool response has text content")
        .contains("matching claims"));
    let claims_json = &claims["result"]["structuredContent"];
    assert_eq!(claims_json["schema"], "texo.mcp.knowledge-search.v2");
    assert_eq!(claims_json["meta"]["workspace_id"], "demo");
    assert!(!claims_json["data"]["claims"]
        .as_array()
        .expect("claims output has claims array")
        .is_empty());
    assert_eq!(claims_json["next_actions"][0]["tool"], "explain_knowledge");
    let snapshot_token = claims_json["meta"]["snapshot"]["token"]
        .as_str()
        .expect("meta carries a snapshot token");
    assert_eq!(
        claims_json["data"]["snapshot"]["token"],
        claims_json["meta"]["snapshot"]["token"]
    );
    assert_eq!(
        claims_json["next_actions"][0]["arguments"]["snapshot_token"],
        snapshot_token
    );

    send_json(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "explain_knowledge",
                "arguments": {
                    "claim_id": "claim:missing",
                    "snapshot_token": snapshot_token
                }
            }
        }),
    )?;
    let explain_error = read_json(&mut stdout)?;
    assert_eq!(explain_error["error"]["code"], -32603);
    assert!(explain_error["error"]["data"]["code"]
        .as_str()
        .expect("op error carries a code token")
        .contains('.'));
    assert!(explain_error["error"]["data"]["committed"].is_string());
    assert!(explain_error["error"]["data"]["retry_safe"].is_boolean());
    assert!(explain_error["error"]["data"].get("resume").is_some());

    assert_triangulation_call(&mut stdin, &mut stdout, snapshot_token)?;

    stdin.write_all(b"{not json\n")?;
    stdin.flush()?;
    let parse_error = read_json(&mut stdout)?;
    assert_eq!(parse_error["error"]["code"], -32700);

    drop(stdin);
    let status = child.wait()?;
    assert!(status.success());
    Ok(())
}

#[test]
fn mcp_input_error_carries_safe_retry_facts() -> TestResult {
    let workspace = TestWorkspace::new()?;
    let root = workspace.root().to_path_buf();
    let support::TestWorkspace { dir: _dir, host } = workspace;
    drop(host);
    let (mut child, mut stdin, mut stdout) = spawn_mcp(&root)?;
    send_json(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "search_knowledge", "arguments": {"limit": 101}}
        }),
    )?;
    let response = read_json(&mut stdout)?;
    assert_eq!(response["error"]["data"]["code"], "op.input");
    assert_eq!(response["error"]["data"]["committed"], "no");
    assert_eq!(response["error"]["data"]["retry_safe"], true);
    assert_eq!(
        response["error"]["data"]["resume"],
        "fix the input and retry"
    );
    drop(stdin);
    assert!(child.wait()?.success());
    Ok(())
}
