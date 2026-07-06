#![cfg(feature = "openrouter")]

//! Loopback tests for the synchronous HTTP client.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::json;
use texo::surfaces::http::client::{request, HttpClientError, HttpRequest, Method, ParsedUrl};
use texo::surfaces::openai::OpenAiCompatClient;

enum Step {
    Respond(Vec<u8>),
    Stall(Duration),
}

fn start_server(steps: Vec<Step>) -> (String, Arc<AtomicUsize>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback server");
    let addr = listener.local_addr().expect("read loopback server address");
    let count = Arc::new(AtomicUsize::new(0));
    let thread_count = Arc::clone(&count);
    let handle = std::thread::Builder::new()
        .name("texo-http-client-test".to_string())
        .spawn(move || {
            for step in steps {
                let (mut stream, _) = listener.accept().expect("accept client");
                thread_count.fetch_add(1, Ordering::SeqCst);
                let mut buffer = [0_u8; 4096];
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .expect("set read timeout");
                let _ = stream.read(&mut buffer);
                match step {
                    Step::Respond(bytes) => {
                        stream.write_all(&bytes).expect("write response");
                    }
                    Step::Stall(duration) => {
                        std::thread::sleep(duration);
                    }
                }
            }
        })
        .expect("spawn named test server");
    (format!("http://{addr}"), count, handle)
}

fn get(url: &str) -> HttpRequest {
    HttpRequest {
        method: Method::Get,
        url: ParsedUrl::parse(url).expect("parse loopback URL"),
        headers: Vec::new(),
        body: Vec::new(),
    }
}

#[test]
fn content_length_response() {
    let (base, count, handle) = start_server(vec![Step::Respond(
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello".to_vec(),
    )]);
    let response = request(&get(&format!("{base}/ok")), Duration::from_secs(5)).expect("request");
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"hello");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    handle.join().expect("server joins");
}

#[test]
fn chunked_response() {
    let (base, _, handle) = start_server(vec![Step::Respond(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n"
            .to_vec(),
    )]);
    let response =
        request(&get(&format!("{base}/chunked")), Duration::from_secs(5)).expect("request");
    assert_eq!(response.body, b"Wikipedia");
    handle.join().expect("server joins");
}

#[test]
fn retry_after_is_honored() {
    let (base, count, handle) = start_server(vec![
        Step::Respond(
            b"HTTP/1.1 429 Too Many\r\nRetry-After: 1\r\nContent-Length: 0\r\n\r\n".to_vec(),
        ),
        Step::Respond(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".to_vec()),
    ]);
    let client =
        OpenAiCompatClient::from_env_vars(Some("key".to_string()), Some(base)).expect("client");
    let started = Instant::now();
    let response = client
        .post_json("/chat/completions", &json!({}))
        .expect("retry succeeds");
    assert_eq!(response["ok"], true);
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(count.load(Ordering::SeqCst), 2);
    handle.join().expect("server joins");
}

#[test]
fn deadline_exceeded() {
    let (base, _, handle) = start_server(vec![Step::Stall(Duration::from_millis(300))]);
    let error = request(&get(&format!("{base}/stall")), Duration::from_millis(50))
        .expect_err("deadline rejected");
    assert!(matches!(error, HttpClientError::DeadlineExceeded));
    handle.join().expect("server joins");
}

#[test]
fn head_too_large() {
    let mut response = Vec::from(b"HTTP/1.1 200 OK\r\nX-Long: ".as_slice());
    response.extend(std::iter::repeat_n(b'a', 9 * 1024));
    response.extend_from_slice(b"\r\n\r\n");
    let (base, _, handle) = start_server(vec![Step::Respond(response)]);
    let error = request(&get(&format!("{base}/big")), Duration::from_secs(5))
        .expect_err("large head rejected");
    assert!(matches!(error, HttpClientError::HeadTooLarge));
    handle.join().expect("server joins");
}
