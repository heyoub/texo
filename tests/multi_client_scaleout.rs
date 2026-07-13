//! Three-client CQRS witness: one canonical journal, three independent MCP replicas.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::json;
use texo::host::TexoHost;
use texo::install::{self, ClientTarget};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[test]
fn three_agent_mcp_processes_read_independent_replicas_concurrently() -> TestResult {
    let root = tempfile::tempdir()?;
    install::install(root.path(), "demo", &[ClientTarget::All], false)?;
    {
        let mut canonical = TexoHost::open_journal(root.path(), "demo", "canonical", 1)?;
        let _ = canonical.invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))?;
        std::fs::create_dir_all(root.path().join("docs"))?;
        std::fs::write(
            root.path().join("docs/decision.md"),
            "Decision: releases happen on Tuesday.\n",
        )?;
        let _ = canonical.invoke_json(
            "texo.ingest.run",
            &json!({"path": "docs", "dry_run": false, "observed_at_ms": 2}),
        )?;
    }
    let mut clients = ["claude", "codex", "cursor"]
        .into_iter()
        .map(|journal| spawn_mcp(root.path(), journal))
        .collect::<Result<Vec<_>, _>>()?;
    for (journal, client) in ["claude", "codex", "cursor"].into_iter().zip(&mut clients) {
        write_request(
            &mut client.stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {"protocolVersion": "2025-06-18"}
            }),
        )?;
        let initialized = read_response(&mut client.stdout)?;
        assert_eq!(initialized["result"]["serverInfo"]["name"], "texo");
        write_request(
            &mut client.stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "get_workspace_status",
                    "arguments": {}
                }
            }),
        )?;
        let status = read_response(&mut client.stdout)?;
        assert_eq!(
            status["result"]["structuredContent"]["meta"]["journal_id"],
            journal
        );
        assert_eq!(
            status["result"]["structuredContent"]["data"]["freshness"],
            "fresh"
        );
    }
    for mut client in clients {
        drop(client.stdin);
        assert!(client.child.wait()?.success());
    }
    let claude: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join(".mcp.json"))?)?;
    assert_eq!(
        claude["mcpServers"]["texo"]["args"][5],
        serde_json::Value::String("claude".to_string())
    );
    let cursor: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join(".cursor/mcp.json"))?)?;
    assert_eq!(
        cursor["mcpServers"]["texo"]["args"][5],
        serde_json::Value::String("cursor".to_string())
    );
    assert!(
        std::fs::read_to_string(root.path().join(".codex/config.toml"))?
            .contains("\"--journal\", \"codex\"")
    );
    Ok(())
}

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

fn spawn_mcp(root: &std::path::Path, journal: &str) -> TestResult<McpClient> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_texo"))
        .arg("--root")
        .arg(root)
        .arg("--workspace")
        .arg("demo")
        .arg("--journal")
        .arg(journal)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let stdin = child.stdin.take().ok_or("MCP stdin")?;
    let stdout = child.stdout.take().ok_or("MCP stdout")?;
    Ok(McpClient {
        child,
        stdin,
        stdout: BufReader::new(stdout),
    })
}

fn write_request(stdin: &mut ChildStdin, request: &serde_json::Value) -> TestResult {
    serde_json::to_writer(&mut *stdin, request)?;
    stdin.write_all(b"\n")?;
    stdin.flush()?;
    Ok(())
}

fn read_response(stdout: &mut BufReader<ChildStdout>) -> TestResult<serde_json::Value> {
    let mut line = String::new();
    if stdout.read_line(&mut line)? == 0 {
        return Err("MCP process closed before responding".into());
    }
    Ok(serde_json::from_str(&line)?)
}
