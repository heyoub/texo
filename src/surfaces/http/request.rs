//! HTTP request parser for the inbound server.

use std::io::Read;

const HEAD_CAP: usize = 8 * 1024;
const BODY_CAP: usize = 1024 * 1024;

/// Supported HTTP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// GET.
    Get,
    /// POST.
    Post,
}

/// Parsed inbound request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// Request method.
    pub method: Method,
    /// Path without query.
    pub path: String,
    /// Query string without `?`.
    pub query: Option<String>,
    /// Headers in arrival order.
    pub headers: Vec<(String, String)>,
    /// Exact request body.
    pub body: Vec<u8>,
}

/// Parser rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestError {
    /// HTTP status code.
    pub status: u16,
    /// JSON error message.
    pub message: String,
    /// Optional Allow header.
    pub allow: Option<&'static str>,
}

impl RequestError {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            allow: None,
        }
    }
}

/// Parse one HTTP/1.x request from a blocking stream.
///
/// # Errors
///
/// Returns [`RequestError`] for protocol rejections or `std::io::Error` for
/// stream read failures.
pub fn parse(stream: &mut impl Read) -> Result<HttpRequest, ParseFailure> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= HEAD_CAP {
            return Err(ParseFailure::Request(RequestError::new(
                400,
                "request head too large",
            )));
        }
        let read = stream.read(&mut byte).map_err(ParseFailure::Io)?;
        if read == 0 {
            return Err(ParseFailure::Request(RequestError::new(
                400,
                "bad request line",
            )));
        }
        head.push(byte[0]);
    }
    let head_text = std::str::from_utf8(&head)
        .map_err(|_| ParseFailure::Request(RequestError::new(400, "request head was not utf-8")))?;
    let mut lines = head_text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = match parts.next() {
        Some("GET") => Method::Get,
        Some("POST") => Method::Post,
        Some(_) => {
            let mut err = RequestError::new(405, "method not allowed");
            err.allow = Some("GET, POST");
            return Err(ParseFailure::Request(err));
        }
        None => {
            return Err(ParseFailure::Request(RequestError::new(
                400,
                "bad request line",
            )));
        }
    };
    let target = parts
        .next()
        .ok_or_else(|| ParseFailure::Request(RequestError::new(400, "bad request line")))?;
    if parts
        .next()
        .is_none_or(|version| !version.starts_with("HTTP/"))
    {
        return Err(ParseFailure::Request(RequestError::new(
            400,
            "bad request line",
        )));
    }
    let (path, query) = split_target(target);
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(ParseFailure::Request(RequestError::new(
                400,
                "malformed header",
            )));
        };
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
    if header(&headers, "transfer-encoding").is_some() {
        return Err(ParseFailure::Request(RequestError::new(
            501,
            "request transfer-encoding is not supported",
        )));
    }
    let mut body = Vec::new();
    if method == Method::Post {
        let len = header(&headers, "content-length")
            .ok_or_else(|| {
                ParseFailure::Request(RequestError::new(411, "content-length required"))
            })?
            .parse::<usize>()
            .map_err(|_| ParseFailure::Request(RequestError::new(400, "bad content-length")))?;
        if len > BODY_CAP {
            return Err(ParseFailure::Request(RequestError::new(
                413,
                "request body too large",
            )));
        }
        body.resize(len, 0);
        stream.read_exact(&mut body).map_err(ParseFailure::Io)?;
    }
    Ok(HttpRequest {
        method,
        path,
        query,
        headers,
        body,
    })
}

/// Parser failure, separating protocol rejection from IO failure.
#[derive(Debug)]
pub enum ParseFailure {
    /// Protocol rejection.
    Request(RequestError),
    /// Stream read failure.
    Io(std::io::Error),
}

/// Case-insensitive header lookup.
#[must_use]
pub fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn split_target(target: &str) -> (String, Option<String>) {
    target.split_once('?').map_or_else(
        || (target.to_string(), None),
        |(path, query)| (path.to_string(), Some(query.to_string())),
    )
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn parse_bytes(bytes: &[u8]) -> Result<HttpRequest, ParseFailure> {
        parse(&mut Cursor::new(bytes))
    }

    #[test]
    fn parses_get_request() {
        let request = parse_bytes(b"GET /api/memory HTTP/1.1\r\nHost: x\r\n\r\n").expect("parse");
        assert_eq!(request.method, Method::Get);
        assert_eq!(request.path, "/api/memory");
    }

    #[test]
    fn rejects_bad_request_line() {
        let err = parse_bytes(b"GET\r\n\r\n").expect_err("bad line");
        assert!(matches!(
            err,
            ParseFailure::Request(RequestError { status: 400, .. })
        ));
    }

    #[test]
    fn rejects_unsupported_method() {
        let err = parse_bytes(b"PUT / HTTP/1.1\r\n\r\n").expect_err("method");
        assert!(matches!(
            err,
            ParseFailure::Request(RequestError { status: 405, .. })
        ));
    }

    #[test]
    fn rejects_request_transfer_encoding() {
        let err =
            parse_bytes(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").expect_err("te");
        assert!(matches!(
            err,
            ParseFailure::Request(RequestError { status: 501, .. })
        ));
    }

    #[test]
    fn rejects_post_without_content_length() {
        let err = parse_bytes(b"POST / HTTP/1.1\r\n\r\n").expect_err("length");
        assert!(matches!(
            err,
            ParseFailure::Request(RequestError { status: 411, .. })
        ));
    }

    #[test]
    fn rejects_large_body_before_reading_it() {
        let head = format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            BODY_CAP + 1
        );
        let err = parse_bytes(head.as_bytes()).expect_err("large");
        assert!(matches!(
            err,
            ParseFailure::Request(RequestError { status: 413, .. })
        ));
    }

    #[test]
    fn rejects_large_head() {
        let mut head = Vec::from(b"GET / HTTP/1.1\r\nX: ".as_slice());
        head.extend(std::iter::repeat_n(b'a', HEAD_CAP + 1));
        head.extend_from_slice(b"\r\n\r\n");
        let err = parse_bytes(&head).expect_err("head");
        assert!(matches!(
            err,
            ParseFailure::Request(RequestError { status: 400, .. })
        ));
    }
}
