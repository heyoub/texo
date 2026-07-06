//! HTTP response helpers for the inbound server.

use std::io::{self, Write};

/// Inbound-server HTTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// Numeric status code.
    pub status: u16,
    /// Extra headers.
    pub headers: Vec<(String, String)>,
    /// Response body bytes.
    pub body: Vec<u8>,
    /// Whether to add `Connection: close`.
    pub close: bool,
}

impl HttpResponse {
    /// Build a response with a byte body.
    #[must_use]
    pub fn new(status: u16, headers: Vec<(String, String)>, body: Vec<u8>) -> Self {
        Self {
            status,
            headers,
            body,
            close: true,
        }
    }

    /// Build a JSON error response.
    #[must_use]
    pub fn json_error(status: u16, message: &str) -> Self {
        let body = serde_json::json!({ "error": message }).to_string();
        Self::new(
            status,
            vec![("Content-Type".to_string(), "application/json".to_string())],
            body.into_bytes(),
        )
    }

    /// Build a JSON success response.
    ///
    /// # Errors
    ///
    /// Returns [`serde_json::Error`] when the value cannot be serialized.
    pub fn json(status: u16, value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        let body = serde_json::to_vec(value)?;
        Ok(Self::new(
            status,
            vec![("Content-Type".to_string(), "application/json".to_string())],
            body,
        ))
    }

    /// Write the response to a stream.
    ///
    /// # Errors
    ///
    /// Returns any stream write error.
    pub fn write_to(&self, stream: &mut impl Write) -> io::Result<()> {
        write!(
            stream,
            "HTTP/1.1 {} {}\r\nContent-Length: {}\r\n",
            self.status,
            reason(self.status),
            self.body.len()
        )?;
        if self.close {
            stream.write_all(b"Connection: close\r\n")?;
        }
        for (name, value) in &self.headers {
            write!(stream, "{name}: {value}\r\n")?;
        }
        stream.write_all(b"\r\n")?;
        stream.write_all(&self.body)
    }
}

/// Content type for a static path.
#[must_use]
pub fn content_type(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "application/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "OK",
    }
}
