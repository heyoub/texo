//! Shared inbound HTTP protocol and server configuration shapes.

use std::time::Duration;

use super::routes::RouteState;

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
    pub(super) fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            allow: None,
        }
    }
}

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

/// Server configuration.
#[derive(Clone)]
pub struct ServerConfig {
    /// Listen address.
    pub addr: String,
    /// Route state.
    pub state: RouteState,
    /// Accept-loop idle sleep.
    pub idle_sleep: Duration,
    /// SSE keep-alive interval.
    pub sse_keep_alive: Duration,
}
