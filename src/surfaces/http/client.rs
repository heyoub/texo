//! Minimal synchronous HTTP/1.1 client.

use std::fmt;
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use rustls::client::ClientConnection;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore, StreamOwned};

use crate::surfaces::http::chunked::decode_chunked;

const HEAD_CAP: usize = 8 * 1024;
const BODY_CAP: usize = 8 * 1024 * 1024;

/// HTTP method supported by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// HTTP GET.
    Get,
    /// HTTP POST.
    Post,
}

impl Method {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// Parsed URL accepted by the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUrl {
    /// URL scheme, either `http` or `https`.
    pub scheme: String,
    /// DNS host, or loopback IP for plain-http tests.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Path plus optional query, always beginning with `/`.
    pub path_and_query: String,
}

impl ParsedUrl {
    /// Parse a minimal `https?://host[:port]/path?query` URL.
    ///
    /// # Errors
    ///
    /// Returns [`HttpClientError::Url`] for unsupported URL syntax and
    /// [`HttpClientError::PlainHttpNonLoopback`] for non-loopback plain HTTP.
    pub fn parse(value: &str) -> Result<Self, HttpClientError> {
        if value.contains('#') {
            return Err(HttpClientError::Url {
                detail: "fragments are not supported".to_string(),
            });
        }
        let (scheme, rest) = value
            .split_once("://")
            .ok_or_else(|| HttpClientError::Url {
                detail: "missing URL scheme".to_string(),
            })?;
        if scheme != "https" && scheme != "http" {
            return Err(HttpClientError::Url {
                detail: "scheme must be http or https".to_string(),
            });
        }
        let (authority, path_and_query) = if let Some((authority, path)) = rest.split_once('/') {
            (authority, format!("/{path}"))
        } else {
            (rest, "/".to_string())
        };
        if authority.is_empty() {
            return Err(HttpClientError::Url {
                detail: "missing host".to_string(),
            });
        }
        if authority.contains('@') {
            return Err(HttpClientError::Url {
                detail: "userinfo is not supported".to_string(),
            });
        }
        if authority.starts_with('[') {
            return Err(HttpClientError::Url {
                detail: "IP-literal hosts are not supported".to_string(),
            });
        }
        let (host, port) = parse_authority(authority, scheme)?;
        if host.parse::<IpAddr>().is_ok() && !(scheme == "http" && is_loopback_host(&host)) {
            return Err(HttpClientError::Url {
                detail: "IP-literal hosts are not supported".to_string(),
            });
        }
        if scheme == "http" && !is_loopback_host(&host) {
            return Err(HttpClientError::PlainHttpNonLoopback);
        }
        Ok(Self {
            scheme: scheme.to_string(),
            host,
            port,
            path_and_query,
        })
    }

    fn authority(&self) -> String {
        let default_port = match self.scheme.as_str() {
            "http" => 80,
            "https" => 443,
            _ => self.port,
        };
        if self.port == default_port {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// HTTP request input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// Request method.
    pub method: Method,
    /// Parsed URL.
    pub url: ParsedUrl,
    /// Caller-provided headers; only `Authorization` is forwarded.
    pub headers: Vec<(String, String)>,
    /// Request body bytes.
    pub body: Vec<u8>,
}

/// HTTP response output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// Numeric HTTP status code.
    pub status: u16,
    /// Response headers.
    pub headers: Vec<(String, String)>,
    /// Response body.
    pub body: Vec<u8>,
}

/// Typed client error.
#[derive(Debug, thiserror::Error)]
pub enum HttpClientError {
    /// URL syntax or policy error.
    #[error("url: {detail}")]
    Url {
        /// Failure detail.
        detail: String,
    },
    /// Plain HTTP was requested for a non-loopback host.
    #[error("plain HTTP is only supported for loopback hosts")]
    PlainHttpNonLoopback,
    /// TCP connect failed.
    #[error("connect: {source}")]
    Connect {
        /// Source I/O error.
        #[source]
        source: io::Error,
    },
    /// TLS setup or handshake failed.
    #[error("tls: {detail}")]
    Tls {
        /// Failure detail.
        detail: String,
    },
    /// I/O failed during a specific phase.
    #[error("io during {during}: {source}")]
    Io {
        /// I/O phase.
        during: &'static str,
        /// Source I/O error.
        #[source]
        source: io::Error,
    },
    /// Wall-clock deadline was exhausted.
    #[error("deadline exceeded")]
    DeadlineExceeded,
    /// Response framing was malformed.
    #[error("malformed response: {detail}")]
    MalformedResponse {
        /// Failure detail.
        detail: String,
    },
    /// Response headers exceeded the 8 KiB cap.
    #[error("response head too large")]
    HeadTooLarge,
    /// Response body exceeded the 8 MiB cap.
    #[error("response body too large")]
    BodyTooLarge,
}

impl HttpClientError {
    /// Whether the error is worth retrying within the caller's deadline.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Connect { .. } | Self::DeadlineExceeded => true,
            Self::Io { source, .. } => matches!(
                source.kind(),
                ErrorKind::TimedOut
                    | ErrorKind::WouldBlock
                    | ErrorKind::ConnectionReset
                    | ErrorKind::BrokenPipe
                    | ErrorKind::Interrupted
            ),
            Self::Url { .. }
            | Self::PlainHttpNonLoopback
            | Self::Tls { .. }
            | Self::MalformedResponse { .. }
            | Self::HeadTooLarge
            | Self::BodyTooLarge => false,
        }
    }
}

/// Execute a request with a wall-clock deadline.
///
/// # Errors
///
/// Returns [`HttpClientError`] for URL policy failures, connect/TLS failures,
/// deadline exhaustion, I/O failures, malformed responses, or configured caps.
pub fn request(req: &HttpRequest, deadline: Duration) -> Result<HttpResponse, HttpClientError> {
    let deadline = Deadline::new(deadline)?;
    let tcp = connect(&req.url, &deadline)?;
    let mut connection = if req.url.scheme == "https" {
        Connection::Tls(Box::new(tls_connect(tcp, &req.url, &deadline)?))
    } else {
        Connection::Plain(tcp)
    };
    write_request(&mut connection, req, &deadline)?;
    let mut reader = BufReader::new(connection);
    read_response(&mut reader, &deadline)
}

fn parse_authority(authority: &str, scheme: &str) -> Result<(String, u16), HttpClientError> {
    let default_port = if scheme == "http" { 80 } else { 443 };
    if let Some((host, port_text)) = authority.rsplit_once(':') {
        if host.is_empty() || port_text.is_empty() {
            return Err(HttpClientError::Url {
                detail: "invalid host or port".to_string(),
            });
        }
        let port = port_text.parse::<u16>().map_err(|_| HttpClientError::Url {
            detail: "invalid port".to_string(),
        })?;
        Ok((host.to_string(), port))
    } else {
        Ok((authority.to_string(), default_port))
    }
}

fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn connect(url: &ParsedUrl, deadline: &Deadline) -> Result<TcpStream, HttpClientError> {
    let addrs = (url.host.as_str(), url.port)
        .to_socket_addrs()
        .map_err(|source| HttpClientError::Connect { source })?;
    let mut last_error = None;
    for addr in addrs {
        let remaining = deadline.remaining()?;
        match TcpStream::connect_timeout(&addr, remaining) {
            Ok(stream) => {
                stream
                    .set_nodelay(true)
                    .map_err(|source| HttpClientError::Io {
                        during: "set nodelay",
                        source,
                    })?;
                return Ok(stream);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(HttpClientError::Connect {
        source: last_error.unwrap_or_else(|| io::Error::other("no socket addresses resolved")),
    })
}

fn tls_config() -> Result<Arc<ClientConfig>, HttpClientError> {
    static CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    let result = CONFIG.get_or_init(|| {
        let root_store = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|error| error.to_string())
            .map(|builder| {
                builder
                    .with_root_certificates(root_store)
                    .with_no_client_auth()
            })
            .map(Arc::new)
    });
    result
        .clone()
        .map_err(|detail| HttpClientError::Tls { detail })
}

fn tls_connect(
    stream: TcpStream,
    url: &ParsedUrl,
    deadline: &Deadline,
) -> Result<StreamOwned<ClientConnection, TcpStream>, HttpClientError> {
    let server_name =
        ServerName::try_from(url.host.clone()).map_err(|error| HttpClientError::Tls {
            detail: error.to_string(),
        })?;
    let connection = ClientConnection::new(tls_config()?, server_name).map_err(|error| {
        HttpClientError::Tls {
            detail: error.to_string(),
        }
    })?;
    let mut stream = StreamOwned::new(connection, stream);
    while stream.conn.is_handshaking() {
        stream
            .sock
            .set_read_timeout(Some(deadline.remaining()?))
            .map_err(|source| HttpClientError::Io {
                during: "tls read timeout",
                source,
            })?;
        stream
            .sock
            .set_write_timeout(Some(deadline.remaining()?))
            .map_err(|source| HttpClientError::Io {
                during: "tls write timeout",
                source,
            })?;
        stream.conn.complete_io(&mut stream.sock).map_err(|error| {
            if is_timeout_error(&error) {
                HttpClientError::DeadlineExceeded
            } else {
                HttpClientError::Tls {
                    detail: error.to_string(),
                }
            }
        })?;
    }
    Ok(stream)
}

fn write_request(
    connection: &mut Connection,
    req: &HttpRequest,
    deadline: &Deadline,
) -> Result<(), HttpClientError> {
    let mut request = Vec::new();
    write!(
        &mut request,
        "{} {} HTTP/1.1\r\n",
        req.method.as_str(),
        req.url.path_and_query
    )
    .expect("writing to a Vec cannot fail");
    write!(&mut request, "Host: {}\r\n", req.url.authority())
        .expect("writing to a Vec cannot fail");
    if let Some((_, value)) = req
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
    {
        write!(&mut request, "Authorization: {value}\r\n").expect("writing to a Vec cannot fail");
    }
    request.extend_from_slice(b"Content-Type: application/json\r\n");
    request.extend_from_slice(b"Accept: application/json\r\n");
    write!(&mut request, "Content-Length: {}\r\n", req.body.len())
        .expect("writing to a Vec cannot fail");
    request.extend_from_slice(b"Connection: close\r\n");
    write!(
        &mut request,
        "User-Agent: texo/{}\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    )
    .expect("writing to a Vec cannot fail");
    request.extend_from_slice(&req.body);

    connection.set_write_timeout(deadline.remaining()?)?;
    connection.write_all(&request).map_err(|source| {
        if is_timeout_error(&source) {
            HttpClientError::DeadlineExceeded
        } else {
            HttpClientError::Io {
                during: "request write",
                source,
            }
        }
    })?;
    connection.flush().map_err(|source| {
        if is_timeout_error(&source) {
            HttpClientError::DeadlineExceeded
        } else {
            HttpClientError::Io {
                during: "request flush",
                source,
            }
        }
    })
}

fn read_response(
    reader: &mut BufReader<Connection>,
    deadline: &Deadline,
) -> Result<HttpResponse, HttpClientError> {
    let (status, headers) = read_head(reader, deadline)?;
    let body = if (100..200).contains(&status) || matches!(status, 204 | 304) {
        Vec::new()
    } else if header_contains(&headers, "transfer-encoding", "chunked") {
        reader.get_mut().set_read_timeout(deadline.remaining()?)?;
        decode_chunked(reader, BODY_CAP)?
    } else if let Some(length) = header_value(&headers, "content-length") {
        let length =
            length
                .trim()
                .parse::<usize>()
                .map_err(|_| HttpClientError::MalformedResponse {
                    detail: "invalid Content-Length".to_string(),
                })?;
        if length > BODY_CAP {
            return Err(HttpClientError::BodyTooLarge);
        }
        let mut body = vec![0_u8; length];
        reader.get_mut().set_read_timeout(deadline.remaining()?)?;
        reader.read_exact(&mut body).map_err(|source| {
            if is_timeout_error(&source) {
                HttpClientError::DeadlineExceeded
            } else if source.kind() == ErrorKind::UnexpectedEof {
                HttpClientError::MalformedResponse {
                    detail: "truncated Content-Length body".to_string(),
                }
            } else {
                HttpClientError::Io {
                    during: "body read",
                    source,
                }
            }
        })?;
        body
    } else {
        read_to_eof(reader, deadline)?
    };
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn read_head(
    reader: &mut BufReader<Connection>,
    deadline: &Deadline,
) -> Result<(u16, Vec<(String, String)>), HttpClientError> {
    let mut head = Vec::new();
    loop {
        if head.len() >= HEAD_CAP {
            return Err(HttpClientError::HeadTooLarge);
        }
        let mut line = Vec::new();
        reader.get_mut().set_read_timeout(deadline.remaining()?)?;
        let read = reader.read_until(b'\n', &mut line).map_err(|source| {
            if is_timeout_error(&source) {
                HttpClientError::DeadlineExceeded
            } else {
                HttpClientError::Io {
                    during: "response head",
                    source,
                }
            }
        })?;
        if read == 0 {
            if deadline.remaining().is_err() {
                return Err(HttpClientError::DeadlineExceeded);
            }
            return Err(HttpClientError::MalformedResponse {
                detail: "response ended before headers".to_string(),
            });
        }
        head.extend_from_slice(&line);
        if head.len() > HEAD_CAP {
            return Err(HttpClientError::HeadTooLarge);
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }
    parse_head(&head)
}

fn parse_head(head: &[u8]) -> Result<(u16, Vec<(String, String)>), HttpClientError> {
    let text = std::str::from_utf8(head).map_err(|_| HttpClientError::MalformedResponse {
        detail: "response head was not UTF-8".to_string(),
    })?;
    let mut lines = text.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| HttpClientError::MalformedResponse {
            detail: "missing status line".to_string(),
        })?;
    let mut parts = status_line.split_whitespace();
    let version = parts
        .next()
        .ok_or_else(|| HttpClientError::MalformedResponse {
            detail: "missing HTTP version".to_string(),
        })?;
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(HttpClientError::MalformedResponse {
            detail: "unsupported HTTP version".to_string(),
        });
    }
    let status = parts
        .next()
        .ok_or_else(|| HttpClientError::MalformedResponse {
            detail: "missing status code".to_string(),
        })?
        .parse::<u16>()
        .map_err(|_| HttpClientError::MalformedResponse {
            detail: "invalid status code".to_string(),
        })?;
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) =
            line.split_once(':')
                .ok_or_else(|| HttpClientError::MalformedResponse {
                    detail: "malformed header".to_string(),
                })?;
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
    Ok((status, headers))
}

fn read_to_eof(
    reader: &mut BufReader<Connection>,
    deadline: &Deadline,
) -> Result<Vec<u8>, HttpClientError> {
    let mut body = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        reader.get_mut().set_read_timeout(deadline.remaining()?)?;
        let read = reader.read(&mut buffer).map_err(|source| {
            if is_timeout_error(&source) {
                HttpClientError::DeadlineExceeded
            } else {
                HttpClientError::Io {
                    during: "body read to EOF",
                    source,
                }
            }
        })?;
        if read == 0 {
            return Ok(body);
        }
        if body.len().saturating_add(read) > BODY_CAP {
            return Err(HttpClientError::BodyTooLarge);
        }
        body.extend_from_slice(&buffer[..read]);
    }
}

fn is_timeout_error(error: &io::Error) -> bool {
    matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn header_contains(headers: &[(String, String)], name: &str, needle: &str) -> bool {
    header_value(headers, name).is_some_and(|value| {
        value
            .split(',')
            .any(|part| part.trim().eq_ignore_ascii_case(needle))
    })
}

struct Deadline {
    at: Instant,
}

impl Deadline {
    fn new(duration: Duration) -> Result<Self, HttpClientError> {
        let at = Instant::now()
            .checked_add(duration)
            .ok_or(HttpClientError::DeadlineExceeded)?;
        let deadline = Self { at };
        deadline.remaining()?;
        Ok(deadline)
    }

    fn remaining(&self) -> Result<Duration, HttpClientError> {
        self.at
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or(HttpClientError::DeadlineExceeded)
    }
}

enum Connection {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Connection {
    fn set_read_timeout(&self, duration: Duration) -> Result<(), HttpClientError> {
        match self {
            Self::Plain(stream) => stream.set_read_timeout(Some(duration)),
            Self::Tls(stream) => stream.sock.set_read_timeout(Some(duration)),
        }
        .map_err(|source| HttpClientError::Io {
            during: "set read timeout",
            source,
        })
    }

    fn set_write_timeout(&self, duration: Duration) -> Result<(), HttpClientError> {
        match self {
            Self::Plain(stream) => stream.set_write_timeout(Some(duration)),
            Self::Tls(stream) => stream.sock.set_write_timeout(Some(duration)),
        }
        .map_err(|source| HttpClientError::Io {
            during: "set write timeout",
            source,
        })
    }
}

impl Read for Connection {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for Connection {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buffer),
            Self::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Connection(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_url() {
        let url =
            ParsedUrl::parse("https://openrouter.ai/api/v1/chat?x=1").expect("valid https URL");
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
}
