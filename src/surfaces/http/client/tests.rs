use super::*;

#[test]
fn parses_https_url() {
    let url = ParsedUrl::parse("https://openrouter.ai/api/v1/chat?x=1").expect("valid https URL");
    assert_eq!(url.scheme, "https");
    assert_eq!(url.host, "openrouter.ai");
    assert_eq!(url.port, 443);
    assert_eq!(url.path_and_query, "/api/v1/chat?x=1");
}

#[test]
fn rejects_plain_http_non_loopback() {
    let error = ParsedUrl::parse("http://example.com/api").expect_err("non-loopback rejected");
    assert!(matches!(error, HttpClientError::PlainHttpNonLoopback));
}

#[test]
fn allows_loopback_plain_http() {
    let url = ParsedUrl::parse("http://127.0.0.1:8080/ok").expect("loopback allowed");
    assert_eq!(url.port, 8080);
}

#[test]
fn rejects_https_ip_literal() {
    let error = ParsedUrl::parse("https://127.0.0.1/api").expect_err("IP literal rejected");
    assert!(matches!(error, HttpClientError::Url { .. }));
}

#[test]
fn deadline_stream_read_loop_cannot_outlive_the_absolute_deadline() {
    // rustls assembles one TLS record by looping small sock.read() calls;
    // a peer trickling bytes resets a socket-level timeout on every byte.
    // This drives DeadlineStream exactly that way and proves the absolute
    // deadline still fires — the property that makes the TLS path safe.
    use std::io::Write as _;
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("addr");
    let server = std::thread::Builder::new()
        .name("deadline-stream-trickle-server".to_string())
        .spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            for _ in 0..10_000 {
                std::thread::sleep(Duration::from_millis(40));
                if stream
                    .write_all(b"x")
                    .and_then(|()| stream.flush())
                    .is_err()
                {
                    break; // client hung up — the intended outcome
                }
            }
        })
        .expect("spawn trickle server");

    let tcp = TcpStream::connect(addr).expect("connect");
    let deadline = Deadline::new(Duration::from_secs(1)).expect("deadline");
    let mut stream = DeadlineStream::new(tcp, &deadline);

    let started = Instant::now();
    let mut scratch = [0_u8; 1];
    let mut total = 0_usize;
    let error = loop {
        match stream.read(&mut scratch) {
            Ok(0) => {
                break io::Error::new(ErrorKind::UnexpectedEof, "peer closed early");
            }
            Ok(n) => {
                total += n;
                assert!(
                    total < 10_000,
                    "drained the whole trickle without timing out"
                );
            }
            Err(error) => break error,
        }
    };
    let elapsed = started.elapsed();
    assert!(
        is_timeout_error(&error),
        "expected a timeout error, got {error:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "read loop ran {elapsed:?} against a 1s deadline"
    );
    drop(stream); // break the pipe so the trickler exits
    let _ = server.join();
}

#[test]
fn deadline_stream_read_fails_closed_once_the_deadline_passed() {
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("addr");
    let server = std::thread::Builder::new()
        .name("deadline-stream-idle-server".to_string())
        .spawn(move || {
            let _accepted = listener.accept();
            std::thread::sleep(Duration::from_millis(200));
        })
        .expect("spawn idle server");

    let tcp = TcpStream::connect(addr).expect("connect");
    // Already-past deadline: the very first read must fail without blocking.
    let deadline = Deadline::new(Duration::from_millis(1)).expect("deadline");
    std::thread::sleep(Duration::from_millis(5));
    let mut stream = DeadlineStream::new(tcp, &deadline);
    let mut scratch = [0_u8; 1];
    let error = stream
        .read(&mut scratch)
        .expect_err("past-deadline read must fail");
    assert!(is_timeout_error(&error), "expected timeout, got {error:?}");
    let _ = server.join();
}

#[test]
fn transient_classification_matches_policy() {
    let timed_out = HttpClientError::Io {
        during: "read",
        source: io::Error::new(ErrorKind::TimedOut, "timeout"),
    };
    assert!(timed_out.is_transient());
    let malformed = HttpClientError::MalformedResponse {
        detail: "bad".to_string(),
    };
    assert!(!malformed.is_transient());
}
