//! Sync HTTP server coverage for WO-4.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use batpak::store::{Open, Store};
use serde_json::{json, Value};
use tempfile::TempDir;
use texo::events::coordinate::scope_for_workspace;
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

fn response_body(response: &str) -> TestResult<&str> {
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .ok_or_else(|| "HTTP response did not include a body separator".into())
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

    let memory_with_query = request(
        addr,
        "GET /api/memory?x=1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )?;
    assert!(memory_with_query.contains("HTTP/1.1 200 OK"));
    assert!(memory_with_query.contains("\"current\":[]"));

    let host = request(addr, "GET /api/host HTTP/1.1\r\nHost: localhost\r\n\r\n")?;
    assert!(host.contains("HTTP/1.1 200 OK"));
    let host_body: Value = serde_json::from_str(response_body(&host)?)?;
    assert_eq!(host_body["schema"], "texo-canonical-v1");
    assert_eq!(host_body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(host_body["workspace_id"], "demo");
    assert!(!host_body["fingerprint"]
        .as_str()
        .expect("host response carries fingerprint")
        .is_empty());

    let host_post = request(
        addr,
        "POST /api/host HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n",
    )?;
    assert!(host_post.contains("HTTP/1.1 405 Method Not Allowed"));
    assert!(host_post.contains("Allow: GET"));

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
    assert!(stats.accepted >= 7);
    Ok(())
}

#[test]
fn sse_streams_journal_signal_and_keep_alive() -> TestResult {
    let dir = TempDir::new()?;
    init_workspace(&dir)?;
    let (addr, shutdown, handle, store) = start_server(&dir, Duration::from_millis(50))?;

    let (mut reader, headers) = open_sse(addr, "/api/stream", &[])?;
    assert!(headers.contains("Content-Type: text/event-stream; charset=utf-8"));
    assert!(headers.contains("Cache-Control: no-cache, no-transform"));
    assert!(!headers.contains("Connection: close"));

    let hello = read_sse_frame(&mut reader)?;
    let hello_frame = sse_data(&hello)?;
    assert_eq!(hello_frame["type"], "signal");
    assert_eq!(hello_frame["data"]["kind"], "hello");
    assert!(!hello_frame["data"]["fingerprint"]
        .as_str()
        .expect("hello frame carries fingerprint")
        .is_empty());

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

#[test]
fn sse_replays_existing_entries_after_query_cursor() -> TestResult {
    let dir = TempDir::new()?;
    init_workspace(&dir)?;
    let (addr, shutdown, handle, store) = start_server(&dir, Duration::from_millis(50))?;
    let expected_sequences = workspace_sequences(&store, "demo");
    assert!(!expected_sequences.is_empty());

    let (mut reader, _headers) = open_sse(addr, "/api/stream?lastEventId=0", &[])?;
    let hello = sse_data(&read_sse_frame(&mut reader)?)?;
    assert_eq!(hello["data"]["kind"], "hello");
    let replayed = read_until_keep_alive(&mut reader)?;
    let replayed_sequences = replayed
        .iter()
        .map(|(sequence, _)| *sequence)
        .collect::<Vec<_>>();
    assert_eq!(replayed_sequences, expected_sequences);
    assert!(replayed_sequences
        .windows(2)
        .all(|window| window[0] < window[1]));
    drop(reader);

    let stats = stop_server(&shutdown, handle)?;
    assert!(stats.accepted >= 1);
    Ok(())
}

#[test]
fn sse_frontier_cursor_and_invalid_query_do_not_replay() -> TestResult {
    let dir = TempDir::new()?;
    init_workspace(&dir)?;
    let (addr, shutdown, handle, store) = start_server(&dir, Duration::from_millis(50))?;
    let frontier = workspace_sequences(&store, "demo")
        .last()
        .copied()
        .unwrap_or(0);
    let frontier_header = frontier.to_string();

    let (mut at_frontier, _headers) =
        open_sse(addr, "/api/stream", &[("Last-Event-ID", &frontier_header)])?;
    let hello = sse_data(&read_sse_frame(&mut at_frontier)?)?;
    assert_eq!(hello["data"]["kind"], "hello");
    assert!(read_until_keep_alive(&mut at_frontier)?.is_empty());
    drop(at_frontier);

    let (mut invalid_query, _headers) = open_sse(addr, "/api/stream?lastEventId=abc", &[])?;
    let hello = sse_data(&read_sse_frame(&mut invalid_query)?)?;
    assert_eq!(hello["data"]["kind"], "hello");
    assert!(read_until_keep_alive(&mut invalid_query)?.is_empty());
    drop(invalid_query);

    let stats = stop_server(&shutdown, handle)?;
    assert!(stats.accepted >= 2);
    Ok(())
}

fn open_sse(
    addr: SocketAddr,
    target: &str,
    extra_headers: &[(&str, &str)],
) -> TestResult<(BufReader<TcpStream>, String)> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut request = format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n");
    for (name, value) in extra_headers {
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes())?;
    let mut reader = BufReader::new(stream);
    let headers = read_sse_headers(&mut reader)?;
    Ok((reader, headers))
}

fn read_sse_headers(reader: &mut BufReader<TcpStream>) -> TestResult<String> {
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
    Ok(headers)
}

fn read_sse_frame(reader: &mut BufReader<TcpStream>) -> TestResult<String> {
    let mut frame = String::new();
    for _ in 0..128 {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Err("stream closed before expected frame".into());
        }
        if line == "\n" || line == "\r\n" {
            if !frame.is_empty() {
                return Ok(frame);
            }
        } else {
            frame.push_str(&line);
        }
    }
    Err("did not observe a complete SSE frame".into())
}

fn read_until_keep_alive(reader: &mut BufReader<TcpStream>) -> TestResult<Vec<(u64, Value)>> {
    let mut replayed = Vec::new();
    for _ in 0..128 {
        let frame = read_sse_frame(reader)?;
        if frame.contains(": keep-alive") {
            return Ok(replayed);
        }
        replayed.push(journal_frame(&frame)?);
    }
    Err("did not observe keep-alive after replay window".into())
}

fn journal_frame(frame: &str) -> TestResult<(u64, Value)> {
    let id = frame
        .lines()
        .find_map(|line| line.strip_prefix("id: "))
        .ok_or_else(|| "SSE journal frame did not include an id line".to_string())?
        .parse::<u64>()?;
    let data = sse_data(frame)?;
    assert_eq!(data["type"], "signal");
    assert_eq!(data["data"]["kind"], "journal");
    assert_eq!(data["data"]["sequence"], id);
    Ok((id, data))
}

fn workspace_sequences(store: &Store<Open>, workspace_id: &str) -> Vec<u64> {
    let scope = scope_for_workspace(workspace_id);
    let mut sequences = store
        .by_scope(&scope)
        .into_iter()
        .map(|entry| entry.global_sequence())
        .collect::<Vec<_>>();
    sequences.sort_unstable();
    sequences
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

fn sse_data(frame: &str) -> TestResult<Value> {
    let data = frame
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .ok_or_else(|| "SSE frame did not include a data line".to_string())?;
    Ok(serde_json::from_str(data)?)
}
