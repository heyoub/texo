//! Sync HTTP server coverage for WO-4.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use batpak::store::{Open, Store};
use serde_json::json;
use tempfile::TempDir;
use texo::host::{open_workspace_store, TexoHost};
use texo::surfaces::http::routes::RouteState;
use texo::surfaces::http::server::{serve_listener, ServeStats, ServerConfig, ShutdownHandle};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
type ServerThread = JoinHandle<Result<ServeStats, texo::error::TexoError>>;
type StartedServer = (SocketAddr, ShutdownHandle, ServerThread, Arc<Store<Open>>);

fn start_server(dir: &TempDir, keep_alive: Duration) -> TestResult<StartedServer> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let store = open_workspace_store(dir.path(), "demo")?;
    let mut config = ServerConfig::new(
        addr.to_string(),
        RouteState {
            root: dir.path().to_path_buf(),
            workspace_id: "demo".to_string(),
            store: Some(Arc::clone(&store)),
            chat_enabled: false,
        },
    );
    config.idle_sleep = Duration::from_millis(5);
    config.sse_keep_alive = keep_alive;
    let shutdown = ShutdownHandle::new();
    let server_shutdown = shutdown.clone();
    let handle = std::thread::Builder::new()
        .name("texo-http-test".to_string())
        .spawn(move || serve_listener(listener, config, &server_shutdown))?;
    Ok((addr, shutdown, handle, store))
}

fn stop_server(shutdown: &ShutdownHandle, handle: ServerThread) -> TestResult<ServeStats> {
    shutdown.shutdown();
    handle
        .join()
        .map_err(|_| "server thread panicked".into())
        .and_then(|result| result.map_err(Into::into))
}

fn request(addr: SocketAddr, request: &str) -> TestResult<String> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(request.as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

fn init_workspace(dir: &TempDir) -> TestResult {
    let mut host = TexoHost::open(dir.path(), "demo", 1)?;
    let _output = host.invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))?;
    Ok(())
}

#[test]
fn agent_flow_guards_and_session_end_idempotence() -> TestResult {
    let dir = TempDir::new()?;
    init_workspace(&dir)?;
    let (addr, shutdown, handle, _store) = start_server(&dir, Duration::from_millis(50))?;

    let memory = request(addr, "GET /api/memory HTTP/1.1\r\nHost: localhost\r\n\r\n")?;
    assert!(memory.contains("HTTP/1.1 200 OK"));
    assert!(memory.contains("\"current\":[]"));

    let invalid = request(
        addr,
        "POST /api/chat HTTP/1.1\r\nHost: localhost\r\nContent-Length: 41\r\n\r\n{\"session_id\":\"bad id\",\"message\":\"hello\"}",
    )?;
    assert!(invalid.contains("HTTP/1.1 400 Bad Request"));
    assert!(invalid.contains("invalid session_id"));

    let disabled = request(
        addr,
        "POST /api/chat HTTP/1.1\r\nHost: localhost\r\nContent-Length: 44\r\n\r\n{\"session_id\":\"s1\",\"message\":\"hello memory\"}",
    )?;
    assert!(disabled.contains("HTTP/1.1 503 Service Unavailable"));
    assert!(disabled.contains("chat is disabled: OPENROUTER_API_KEY is not set"));

    let missing = request(
        addr,
        "POST /api/session/end HTTP/1.1\r\nHost: localhost\r\nContent-Length: 24\r\n\r\n{\"session_id\":\"unknown\"}",
    )?;
    assert!(missing.contains("HTTP/1.1 404 Not Found"));
    assert!(missing.contains("unknown or empty session: unknown"));

    let stats = stop_server(&shutdown, handle)?;
    assert!(stats.accepted >= 4);
    Ok(())
}

#[test]
fn sse_streams_journal_signal_and_keep_alive() -> TestResult {
    let dir = TempDir::new()?;
    init_workspace(&dir)?;
    let (addr, shutdown, handle, store) = start_server(&dir, Duration::from_millis(50))?;

    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"GET /api/stream?lastEventId=0 HTTP/1.1\r\nHost: localhost\r\n\r\n")?;
    let mut reader = BufReader::new(stream);

    let mut line = String::new();
    reader.read_line(&mut line)?;
    assert!(line.starts_with("HTTP/1.1 200 OK"));
    let mut headers = String::new();
    loop {
        line.clear();
        reader.read_line(&mut line)?;
        if line == "\r\n" {
            break;
        }
        headers.push_str(&line);
    }
    assert!(headers.contains("Content-Type: text/event-stream; charset=utf-8"));
    assert!(headers.contains("Cache-Control: no-cache, no-transform"));
    assert!(!headers.contains("Connection: close"));

    let hello = read_until(&mut reader, "\"kind\":\"hello\"")?;
    assert!(hello.contains("\"type\":\"signal\""));

    let mut append_host = TexoHost::open_with_store(dir.path(), "demo", 2, store)?;
    let _output =
        append_host.invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))?;

    let journal = read_until(&mut reader, "\"kind\":\"journal\"")?;
    assert!(journal.contains("id: "));
    assert!(journal.contains("\"sequence\":"));
    assert!(journal.contains("\"kind_bits\":"));

    let keep_alive = read_until(&mut reader, ": keep-alive")?;
    assert!(keep_alive.contains(": keep-alive"));
    drop(reader);

    let stats = stop_server(&shutdown, handle)?;
    assert!(stats.accepted >= 1);
    Ok(())
}

fn read_until(reader: &mut BufReader<TcpStream>, needle: &str) -> TestResult<String> {
    let mut collected = String::new();
    for _ in 0..128 {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Err("stream closed before expected frame".into());
        }
        collected.push_str(&line);
        if collected.contains(needle) {
            return Ok(collected);
        }
    }
    Err(format!("did not observe expected stream fragment: {needle}").into())
}
