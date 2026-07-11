//! OpenAI-compatible JSON transport used by every model role.

use std::fmt;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::gateway::ResolvedRole;
use crate::surfaces::http::client::{request, HttpClientError, HttpRequest, Method, ParsedUrl};
use crate::surfaces::http::retry::{parse_retry_after, retry_delay};

const MAX_PROVIDER_MESSAGE: usize = 200;

/// Closed classification of an OpenAI-compatible request failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiFailureKind {
    /// The provider returned a non-success HTTP status.
    HttpStatus,
    /// The HTTP transport failed.
    Transport,
    /// The configured wall-clock request budget expired.
    DeadlineExceeded,
    /// A successful response was not valid JSON.
    BadResponseJson,
}

impl fmt::Display for ApiFailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::HttpStatus => "HTTP status",
            Self::Transport => "transport",
            Self::DeadlineExceeded => "deadline exceeded",
            Self::BadResponseJson => "invalid response JSON",
        };
        formatter.write_str(value)
    }
}

/// Sanitized, diagnosable transport failure.
#[derive(Debug, thiserror::Error)]
#[error("{endpoint} {kind}{status_text} after {attempts} attempt(s){message_text}")]
pub struct ApiFailure {
    /// Endpoint path.
    pub endpoint: &'static str,
    /// Stable failure class.
    pub kind: ApiFailureKind,
    /// HTTP status, when a response was received.
    pub status: Option<u16>,
    /// Total attempts including the initial request.
    pub attempts: u32,
    /// Redacted provider message capped at 200 characters.
    pub provider_message: Option<String>,
    status_text: StatusText,
    message_text: MessageText,
}

#[derive(Debug)]
struct StatusText(Option<u16>);

impl fmt::Display for StatusText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
            .map_or(Ok(()), |status| write!(formatter, " {status}"))
    }
}

#[derive(Debug)]
struct MessageText(Option<String>);

impl fmt::Display for MessageText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
            .as_deref()
            .map_or(Ok(()), |message| write!(formatter, ": {message}"))
    }
}

impl ApiFailure {
    fn new(
        endpoint: &'static str,
        kind: ApiFailureKind,
        status: Option<u16>,
        attempts: u32,
        provider_message: Option<String>,
        api_key: &str,
    ) -> Self {
        let provider_message = provider_message
            .map(|message| redact_and_cap(&message, api_key))
            .filter(|message| !message.is_empty());
        Self {
            endpoint,
            kind,
            status,
            attempts,
            status_text: StatusText(status),
            message_text: MessageText(provider_message.clone()),
            provider_message,
        }
    }
}

/// OpenAI-compatible JSON client built only from a resolved gateway role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCompatClient {
    base_url: String,
    api_key: String,
    retry_max: u32,
    request_timeout: Duration,
}

impl OpenAiCompatClient {
    /// Build a client from an already-resolved model role.
    ///
    /// # Errors
    /// Returns [`ApiFailureKind::Transport`] when the role has no key or its
    /// base URL is rejected by the local HTTP client.
    pub fn from_role(role: &ResolvedRole) -> Result<Self, ApiFailure> {
        Self::new(
            &role.profile.base_url,
            role.api_key.clone(),
            role.profile.retry_max,
            Duration::from_secs(role.profile.request_timeout_secs),
        )
    }

    /// Build from explicit resolved values. This is also the loopback-test seam.
    ///
    /// # Errors
    /// Returns [`ApiFailureKind::Transport`] for a missing key or invalid URL.
    pub fn new(
        base_url: &str,
        api_key: String,
        retry_max: u32,
        request_timeout: Duration,
    ) -> Result<Self, ApiFailure> {
        let endpoint = "/client";
        if api_key.trim().is_empty() {
            return Err(ApiFailure::new(
                endpoint,
                ApiFailureKind::Transport,
                None,
                0,
                Some("missing model API key".to_string()),
                &api_key,
            ));
        }
        let base_url = base_url.trim_end_matches('/').to_string();
        ParsedUrl::parse(&format!("{base_url}/models")).map_err(|error| {
            ApiFailure::new(
                endpoint,
                ApiFailureKind::Transport,
                None,
                0,
                Some(error.to_string()),
                &api_key,
            )
        })?;
        Ok(Self {
            base_url,
            api_key,
            retry_max,
            request_timeout,
        })
    }

    /// POST JSON and decode the JSON response.
    ///
    /// # Errors
    /// Returns a typed [`ApiFailure`] for status, transport, deadline, or JSON
    /// response failures. Provider bodies never enter the error unchanged.
    pub fn post_json(&self, endpoint: &'static str, body: &Value) -> Result<Value, ApiFailure> {
        let deadline_at = Instant::now()
            .checked_add(self.request_timeout)
            .ok_or_else(|| {
                self.failure(endpoint, ApiFailureKind::DeadlineExceeded, None, 0, None)
            })?;
        let payload = serde_json::to_vec(body).map_err(|error| {
            self.failure(
                endpoint,
                ApiFailureKind::BadResponseJson,
                None,
                0,
                Some(error.to_string()),
            )
        })?;
        let mut retry_count = 0_u32;
        loop {
            let attempts = retry_count.saturating_add(1);
            let remaining = self.remaining_budget(endpoint, deadline_at, attempts)?;
            let url =
                ParsedUrl::parse(&format!("{}{}", self.base_url, endpoint)).map_err(|error| {
                    self.failure(
                        endpoint,
                        ApiFailureKind::Transport,
                        None,
                        attempts,
                        Some(error.to_string()),
                    )
                })?;
            let request_value = HttpRequest {
                method: Method::Post,
                url,
                headers: vec![(
                    "Authorization".to_string(),
                    format!("Bearer {}", self.api_key),
                )],
                body: payload.clone(),
            };
            match request(&request_value, remaining) {
                Ok(response) if (200..300).contains(&response.status) => {
                    return serde_json::from_slice(&response.body).map_err(|error| {
                        self.failure(
                            endpoint,
                            ApiFailureKind::BadResponseJson,
                            Some(response.status),
                            attempts,
                            Some(error.to_string()),
                        )
                    });
                }
                Ok(response) => {
                    if retryable_status(response.status) && retry_count < self.retry_max {
                        retry_count = retry_count.saturating_add(1);
                        self.sleep_before_retry(
                            endpoint,
                            retry_delay(retry_count, parse_retry_after(&response.headers)),
                            deadline_at,
                            attempts,
                        )?;
                        continue;
                    }
                    let raw = String::from_utf8_lossy(&response.body);
                    let debug_body = redact_and_cap(&raw, &self.api_key);
                    tracing::debug!(endpoint, status = response.status, body = %debug_body, "model provider returned an error response");
                    let message = provider_error_message(&response.body)
                        .unwrap_or_else(|| format!("provider returned HTTP {}", response.status));
                    return Err(self.failure(
                        endpoint,
                        ApiFailureKind::HttpStatus,
                        Some(response.status),
                        attempts,
                        Some(message),
                    ));
                }
                Err(error) => {
                    if error.is_transient() && retry_count < self.retry_max {
                        retry_count = retry_count.saturating_add(1);
                        self.sleep_before_retry(
                            endpoint,
                            retry_delay(retry_count, None),
                            deadline_at,
                            attempts,
                        )?;
                        continue;
                    }
                    return Err(self.transport_failure(endpoint, &error, attempts));
                }
            }
        }
    }

    fn failure(
        &self,
        endpoint: &'static str,
        kind: ApiFailureKind,
        status: Option<u16>,
        attempts: u32,
        message: Option<String>,
    ) -> ApiFailure {
        ApiFailure::new(endpoint, kind, status, attempts, message, &self.api_key)
    }

    fn transport_failure(
        &self,
        endpoint: &'static str,
        error: &HttpClientError,
        attempts: u32,
    ) -> ApiFailure {
        self.failure(
            endpoint,
            ApiFailureKind::Transport,
            None,
            attempts,
            Some(error.to_string()),
        )
    }

    fn remaining_budget(
        &self,
        endpoint: &'static str,
        deadline_at: Instant,
        attempts: u32,
    ) -> Result<Duration, ApiFailure> {
        deadline_at
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| {
                self.failure(
                    endpoint,
                    ApiFailureKind::DeadlineExceeded,
                    None,
                    attempts,
                    None,
                )
            })
    }

    fn sleep_before_retry(
        &self,
        endpoint: &'static str,
        delay: Duration,
        deadline_at: Instant,
        attempts: u32,
    ) -> Result<(), ApiFailure> {
        let remaining = self.remaining_budget(endpoint, deadline_at, attempts)?;
        if delay >= remaining {
            return Err(self.failure(
                endpoint,
                ApiFailureKind::DeadlineExceeded,
                None,
                attempts,
                None,
            ));
        }
        std::thread::sleep(delay);
        Ok(())
    }
}

fn retryable_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

fn provider_error_message(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn redact_and_cap(value: &str, api_key: &str) -> String {
    let exact = if api_key.is_empty() {
        value.to_string()
    } else {
        value.replace(api_key, "[redacted]")
    };
    let bearer_redacted = redact_bearer(&exact);
    let secret_redacted = redact_sk_tokens(&bearer_redacted);
    secret_redacted.chars().take(MAX_PROVIDER_MESSAGE).collect()
}

fn redact_bearer(value: &str) -> String {
    redact_prefixed_token(value, "Bearer ", |_| true)
}

fn redact_sk_tokens(value: &str) -> String {
    redact_prefixed_token(value, "sk-", |token| {
        token.len() >= 11
            && token
                .bytes()
                .skip(3)
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

fn redact_prefixed_token(
    value: &str,
    prefix: &str,
    should_redact: impl Fn(&str) -> bool,
) -> String {
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start) = remaining.find(prefix) {
        output.push_str(&remaining[..start]);
        let token_start = start + prefix.len();
        let token_len = remaining[token_start..]
            .find(char::is_whitespace)
            .unwrap_or(remaining.len() - token_start);
        let end = token_start + token_len;
        let token = &remaining[start..end];
        if should_redact(token) {
            output.push_str("[redacted]");
        } else {
            output.push_str(token);
        }
        remaining = &remaining[end..];
    }
    output.push_str(remaining);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_rejects_missing_key_and_bad_base() {
        let missing = OpenAiCompatClient::new(
            "https://example.com/v1",
            String::new(),
            0,
            Duration::from_secs(1),
        )
        .expect_err("missing key rejected");
        assert_eq!(missing.kind, ApiFailureKind::Transport);

        let bad = OpenAiCompatClient::new(
            "http://example.com/v1",
            "key".to_string(),
            0,
            Duration::from_secs(1),
        )
        .expect_err("plain HTTP rejected");
        assert!(bad.to_string().contains("plain HTTP"));
    }

    #[test]
    fn provider_message_redacts_before_capping() {
        let key = "sk-exact_secret_123";
        let raw = format!(
            "Bearer token-value {key} sk-another_secret_456 {}",
            "x".repeat(300)
        );
        let redacted = redact_and_cap(&raw, key);
        assert!(!redacted.contains("Bearer"));
        assert!(!redacted.contains("sk-"));
        assert!(!redacted.contains("exact_secret"));
        assert!(redacted.chars().count() <= MAX_PROVIDER_MESSAGE);
    }
}
