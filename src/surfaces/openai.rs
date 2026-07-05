//! OpenAI-compatible JSON edge over the local HTTP client.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::error::TexoError;
use crate::surfaces::http::client::{request, HttpClientError, HttpRequest, Method, ParsedUrl};
use crate::surfaces::http::retry::{parse_retry_after, retry_delay, MAX_RETRIES};

const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
const ENV_API_KEY: &str = "OPENROUTER_API_KEY";
const ENV_BASE_URL: &str = "OPENROUTER_BASE_URL";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_ERROR_BODY: usize = 2048;

/// OpenAI-compatible JSON client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCompatClient {
    base_url: String,
    api_key: String,
}

impl OpenAiCompatClient {
    /// Build a client from resolved environment variable values.
    ///
    /// # Errors
    ///
    /// Returns [`TexoError::Semantics`] when the API key is missing or the base
    /// URL is rejected by the HTTP client URL parser.
    pub fn from_env_vars(key: Option<String>, base: Option<String>) -> Result<Self, TexoError> {
        let api_key = key
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| semantics_error("missing OPENROUTER_API_KEY"))?;
        let base_url = base
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        ParsedUrl::parse(&format!("{base_url}/models"))
            .map_err(|error| semantics_http_error(&error))?;
        Ok(Self { base_url, api_key })
    }

    /// Build a client by reading `OPENROUTER_API_KEY` and `OPENROUTER_BASE_URL`.
    ///
    /// # Errors
    ///
    /// Returns [`TexoError::Semantics`] when the API key is absent or the base
    /// URL is invalid.
    pub fn from_env() -> Result<Self, TexoError> {
        Self::from_env_vars(
            std::env::var(ENV_API_KEY).ok(),
            std::env::var(ENV_BASE_URL).ok(),
        )
    }

    /// POST JSON to an endpoint and parse a JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`TexoError::Semantics`] for transport errors, non-success HTTP
    /// statuses, malformed URLs, or invalid JSON responses.
    pub fn post_json(&self, endpoint: &'static str, body: &Value) -> Result<Value, TexoError> {
        let deadline_at = Instant::now()
            .checked_add(REQUEST_TIMEOUT)
            .ok_or_else(|| semantics_error("request deadline overflow"))?;
        let payload = serde_json::to_vec(body)?;
        let mut attempt = 0_u32;
        loop {
            let remaining = remaining_budget(deadline_at)?;
            let url = ParsedUrl::parse(&format!("{}{}", self.base_url, endpoint))
                .map_err(|error| semantics_http_error(&error))?;
            let req = HttpRequest {
                method: Method::Post,
                url,
                headers: vec![(
                    "Authorization".to_string(),
                    format!("Bearer {}", self.api_key),
                )],
                body: payload.clone(),
            };

            match request(&req, remaining) {
                Ok(response) if (200..300).contains(&response.status) => {
                    return serde_json::from_slice(&response.body).map_err(|source| {
                        TexoError::Semantics {
                            backend: "openai-compat".to_string(),
                            detail: format!(
                                "could not parse OpenRouter {endpoint} response: {source}"
                            ),
                        }
                    });
                }
                Ok(response) => {
                    if is_retryable_status(response.status) && attempt < MAX_RETRIES {
                        attempt = attempt.saturating_add(1);
                        let delay = retry_delay(attempt, parse_retry_after(&response.headers));
                        sleep_before_retry(delay, deadline_at)?;
                        continue;
                    }
                    let body = truncated_body(&response.body);
                    return Err(TexoError::Semantics {
                        backend: "openai-compat".to_string(),
                        detail: format!(
                            "OpenRouter {endpoint} returned HTTP {}: {body}",
                            response.status
                        ),
                    });
                }
                Err(error) => {
                    if error.is_transient() && attempt < MAX_RETRIES {
                        attempt = attempt.saturating_add(1);
                        sleep_before_retry(retry_delay(attempt, None), deadline_at)?;
                        continue;
                    }
                    return Err(semantics_http_error(&error));
                }
            }
        }
    }
}

fn is_retryable_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

fn remaining_budget(deadline_at: Instant) -> Result<Duration, TexoError> {
    deadline_at
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| semantics_error("deadline exceeded"))
}

fn sleep_before_retry(delay: Duration, deadline_at: Instant) -> Result<(), TexoError> {
    let remaining = remaining_budget(deadline_at)?;
    if delay >= remaining {
        std::thread::sleep(remaining);
        return Err(semantics_error("deadline exceeded"));
    }
    std::thread::sleep(delay);
    Ok(())
}

fn truncated_body(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    text.chars().take(MAX_ERROR_BODY).collect()
}

fn semantics_http_error(error: &HttpClientError) -> TexoError {
    TexoError::Semantics {
        backend: "openai-compat".to_string(),
        detail: error.to_string(),
    }
}

fn semantics_error(detail: impl Into<String>) -> TexoError {
    TexoError::Semantics {
        backend: "openai-compat".to_string(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_vars_uses_default_base_and_trims_override() {
        let defaulted =
            OpenAiCompatClient::from_env_vars(Some("k".to_string()), None).expect("client");
        assert_eq!(defaulted.base_url, DEFAULT_BASE_URL);

        let custom = OpenAiCompatClient::from_env_vars(
            Some("k".to_string()),
            Some("https://example.com/api/".to_string()),
        )
        .expect("client");
        assert_eq!(custom.base_url, "https://example.com/api");
    }

    #[test]
    fn from_env_vars_rejects_missing_key() {
        let error = OpenAiCompatClient::from_env_vars(Some(" ".to_string()), None)
            .expect_err("missing key rejected");
        assert!(matches!(error, TexoError::Semantics { .. }));
        assert!(error.to_string().contains("OPENROUTER_API_KEY"));
    }

    #[test]
    fn from_env_vars_rejects_bad_base() {
        let error = OpenAiCompatClient::from_env_vars(
            Some("k".to_string()),
            Some("http://example.com/api".to_string()),
        )
        .expect_err("plain HTTP rejected");
        assert!(error.to_string().contains("plain HTTP"));
    }

    #[test]
    fn truncates_error_body() {
        let body = vec![b'a'; MAX_ERROR_BODY + 20];
        assert_eq!(truncated_body(&body).len(), MAX_ERROR_BODY);
    }
}
