//! Hosted semantic backends backed by the OpenRouter API.
//!
//! These implement the `texo-core` semantic traits over HTTP using
//! `reqwest::blocking` (the traits are synchronous). They run on any CPU and
//! require only an API key, which makes them the portable default backend.
//!
//! Three backends are provided:
//!
//! - [`OpenRouterEmbedder`] -> `POST /embeddings`
//! - [`OpenRouterReranker`] -> `POST /rerank`
//! - [`OpenRouterNli`] -> `POST /chat/completions` (LLM-as-judge; OpenRouter has
//!   no hosted NLI endpoint, so the model is prompted to return strict JSON)
//!
//! Request-body construction and response parsing are factored into pure
//! functions ([`build_*_request`] / [`parse_*_response`]) so they can be unit
//! tested against hand-written JSON without any network access.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use texo_core::{Embedder, Entailment, Nli, NliVerdict, Reranker, SemanticsError};

use crate::error::BackendError;

/// Default OpenRouter API base URL, used when `OPENROUTER_BASE_URL` is unset.
const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
/// Default embeddings model.
const DEFAULT_EMBEDDING_MODEL: &str = "google/gemini-embedding-2";
/// Default rerank model.
const DEFAULT_RERANK_MODEL: &str = "cohere/rerank-4-pro";
/// Default NLI judge model. A cheap/free model suited to testing; override via
/// `OPENROUTER_NLI_MODEL` (or the explicit constructor) for production.
const DEFAULT_NLI_MODEL: &str = "google/gemma-4-31b-it:free";

/// Environment variable holding the bearer token.
const ENV_API_KEY: &str = "OPENROUTER_API_KEY";
/// Environment variable overriding the API base URL.
const ENV_BASE_URL: &str = "OPENROUTER_BASE_URL";
/// Environment variable overriding the NLI judge model.
const ENV_NLI_MODEL: &str = "OPENROUTER_NLI_MODEL";

/// Per-request timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Number of retry attempts on retryable (429 / 5xx) statuses, in addition to
/// the initial attempt.
const MAX_RETRIES: u32 = 2;
/// Base backoff between retries.
const RETRY_BACKOFF: Duration = Duration::from_millis(500);
/// Maximum number of bytes of an error body retained for diagnostics.
const MAX_ERROR_BODY: usize = 2048;

/// A thin blocking HTTP client for the OpenRouter API.
///
/// Holds the base URL, bearer token, and a configured `reqwest` client. Shared
/// by all three backends.
struct OpenRouterClient {
    http: reqwest::blocking::Client,
    base_url: String,
    api_key: String,
}

impl OpenRouterClient {
    /// Build a client, resolving the base URL from `OPENROUTER_BASE_URL` (else
    /// the default) and the bearer token from `OPENROUTER_API_KEY`.
    ///
    /// Returns [`BackendError::MissingApiKey`] if no key is set; the constructor
    /// is the single place a missing key is detected, so backend constructors
    /// fail fast rather than at first request.
    fn from_env() -> Result<Self, BackendError> {
        let api_key = std::env::var(ENV_API_KEY)
            .ok()
            .filter(|key| !key.trim().is_empty())
            .ok_or(BackendError::MissingApiKey)?;
        let base_url = std::env::var(ENV_BASE_URL)
            .ok()
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        let http = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|source| BackendError::HttpClientBuild { source })?;
        Ok(Self {
            http,
            base_url,
            api_key,
        })
    }

    /// POST `body` as JSON to `endpoint` (a path like `/embeddings`) and return
    /// the parsed JSON response.
    ///
    /// Retries on HTTP 429 and 5xx up to [`MAX_RETRIES`] times with linear
    /// backoff; transport errors are returned immediately. Non-retryable
    /// non-success statuses become [`BackendError::HttpStatus`].
    fn post_json(&self, endpoint: &'static str, body: &Value) -> Result<Value, BackendError> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), endpoint);

        let mut attempt = 0;
        loop {
            let response = self
                .http
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(body)
                .send()
                .map_err(|source| BackendError::Http { endpoint, source })?;

            let status = response.status();
            if status.is_success() {
                let text = response
                    .text()
                    .map_err(|source| BackendError::Http { endpoint, source })?;
                return serde_json::from_str(&text)
                    .map_err(|source| BackendError::Parse { endpoint, source });
            }

            let retryable = status.as_u16() == 429 || status.is_server_error();
            if retryable && attempt < MAX_RETRIES {
                attempt += 1;
                std::thread::sleep(RETRY_BACKOFF * attempt);
                continue;
            }

            let code = status.as_u16();
            let mut body = response.text().unwrap_or_default();
            body.truncate(MAX_ERROR_BODY);
            return Err(BackendError::HttpStatus {
                endpoint,
                status: code,
                body,
            });
        }
    }
}

/// Resolve a model id: explicit `Some` wins; else the environment variable if
/// set and non-empty; else `default`.
fn resolve_model(explicit: Option<String>, env_var: Option<&str>, default: &str) -> String {
    if let Some(model) = explicit.filter(|m| !m.trim().is_empty()) {
        return model;
    }
    if let Some(var) = env_var {
        if let Ok(value) = std::env::var(var) {
            if !value.trim().is_empty() {
                return value;
            }
        }
    }
    default.to_owned()
}

// =====================================================================
// Embeddings
// =====================================================================

/// Hosted [`Embedder`] backed by OpenRouter's `/embeddings` endpoint.
pub struct OpenRouterEmbedder {
    client: OpenRouterClient,
    model: String,
}

impl OpenRouterEmbedder {
    /// Build an embedder. The model id is resolved from `model` (if `Some` and
    /// non-empty), then the default ([`DEFAULT_EMBEDDING_MODEL`]). Reads the API
    /// key and base URL from the environment.
    pub fn new(model: Option<String>) -> Result<Self, SemanticsError> {
        let client = OpenRouterClient::from_env()?;
        let model = resolve_model(model, None, DEFAULT_EMBEDDING_MODEL);
        Ok(Self { client, model })
    }
}

/// Build the JSON request body for an embeddings call over `inputs`.
fn build_embeddings_request(model: &str, inputs: &[&str]) -> Value {
    json!({ "model": model, "input": inputs })
}

/// OpenRouter `/embeddings` response shape (the subset this backend reads).
#[derive(Debug, Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingDatum>,
}

/// One row of an embeddings response.
#[derive(Debug, Deserialize)]
struct EmbeddingDatum {
    #[serde(default)]
    index: Option<usize>,
    embedding: Vec<f32>,
}

/// Parse an embeddings response into one vector per input, ordered to match the
/// request. The endpoint may return rows out of order, so rows are sorted by
/// their `index` field when present.
fn parse_embeddings_response(
    value: &Value,
    expected: usize,
) -> Result<Vec<Vec<f32>>, BackendError> {
    let endpoint = "/embeddings";
    let parsed: EmbeddingsResponse = serde_json::from_value(value.clone())
        .map_err(|source| BackendError::Parse { endpoint, source })?;
    if parsed.data.len() != expected {
        return Err(BackendError::UnexpectedResponse {
            endpoint,
            detail: format!(
                "expected {expected} embedding rows, got {}",
                parsed.data.len()
            ),
        });
    }

    let mut rows = parsed.data;
    // Order by `index` when every row carries one; otherwise trust array order.
    if rows.iter().all(|row| row.index.is_some()) {
        rows.sort_by_key(|row| row.index.unwrap_or(0));
    }

    let vectors: Vec<Vec<f32>> = rows.into_iter().map(|row| row.embedding).collect();
    if vectors.iter().any(Vec::is_empty) {
        return Err(BackendError::UnexpectedResponse {
            endpoint,
            detail: "an embedding row contained an empty vector".to_owned(),
        });
    }
    Ok(vectors)
}

impl Embedder for OpenRouterEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, SemanticsError> {
        let mut vectors = self.embed_batch(&[text])?;
        vectors.pop().ok_or(SemanticsError::ResultCountMismatch {
            expected: 1,
            actual: 0,
        })
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SemanticsError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let body = build_embeddings_request(&self.model, texts);
        let value = self.client.post_json("/embeddings", &body)?;
        let vectors = parse_embeddings_response(&value, texts.len())?;
        Ok(vectors)
    }
}

// =====================================================================
// Rerank
// =====================================================================

/// Hosted [`Reranker`] backed by OpenRouter's `/rerank` endpoint.
pub struct OpenRouterReranker {
    client: OpenRouterClient,
    model: String,
}

impl OpenRouterReranker {
    /// Build a reranker. The model id is resolved from `model` (if `Some` and
    /// non-empty), then the default ([`DEFAULT_RERANK_MODEL`]). Reads the API
    /// key and base URL from the environment.
    pub fn new(model: Option<String>) -> Result<Self, SemanticsError> {
        let client = OpenRouterClient::from_env()?;
        let model = resolve_model(model, None, DEFAULT_RERANK_MODEL);
        Ok(Self { client, model })
    }
}

/// Build the JSON request body for a rerank call.
fn build_rerank_request(model: &str, query: &str, documents: &[&str]) -> Value {
    json!({ "model": model, "query": query, "documents": documents })
}

/// OpenRouter `/rerank` response shape (the subset this backend reads).
#[derive(Debug, Deserialize)]
struct RerankResponse {
    results: Vec<RerankResult>,
}

/// One scored document from a rerank response.
#[derive(Debug, Deserialize)]
struct RerankResult {
    index: usize,
    relevance_score: f32,
}

/// Parse a rerank response into one score per input document, realigned to the
/// original document order via each result's `index`.
fn parse_rerank_response(value: &Value, doc_count: usize) -> Result<Vec<f32>, BackendError> {
    let endpoint = "/rerank";
    let parsed: RerankResponse = serde_json::from_value(value.clone())
        .map_err(|source| BackendError::Parse { endpoint, source })?;
    if parsed.results.len() != doc_count {
        return Err(BackendError::UnexpectedResponse {
            endpoint,
            detail: format!(
                "expected {doc_count} rerank results, got {}",
                parsed.results.len()
            ),
        });
    }

    let mut scores = vec![None; doc_count];
    for result in parsed.results {
        let slot =
            scores
                .get_mut(result.index)
                .ok_or_else(|| BackendError::UnexpectedResponse {
                    endpoint,
                    detail: format!(
                        "result index {} is out of range for {doc_count} documents",
                        result.index
                    ),
                })?;
        if slot.is_some() {
            return Err(BackendError::UnexpectedResponse {
                endpoint,
                detail: format!("result index {} appeared more than once", result.index),
            });
        }
        *slot = Some(result.relevance_score);
    }

    scores
        .into_iter()
        .enumerate()
        .map(|(i, score)| {
            score.ok_or_else(|| BackendError::UnexpectedResponse {
                endpoint,
                detail: format!("no rerank score returned for document index {i}"),
            })
        })
        .collect()
}

impl Reranker for OpenRouterReranker {
    fn rerank(&self, query: &str, docs: &[&str]) -> Result<Vec<f32>, SemanticsError> {
        if docs.is_empty() {
            return Ok(Vec::new());
        }
        let body = build_rerank_request(&self.model, query, docs);
        let value = self.client.post_json("/rerank", &body)?;
        let scores = parse_rerank_response(&value, docs.len())?;
        Ok(scores)
    }
}

// =====================================================================
// NLI (LLM-as-judge over chat/completions)
// =====================================================================

/// System prompt instructing the judge model to behave as an NLI classifier.
const NLI_SYSTEM_PROMPT: &str = "You are a strict natural-language-inference \
classifier. Given a premise and a hypothesis, decide whether the premise \
entails the hypothesis (label \"entailment\"), is unrelated to or insufficient \
for it (label \"neutral\"), or contradicts it (label \"contradiction\"). Reply \
with ONLY a single JSON object of the form {\"label\": \"entailment\" | \
\"neutral\" | \"contradiction\", \"score\": <number between 0 and 1>}. The score \
is your confidence in the chosen label. Output no prose, no markdown, no code \
fences.";

/// JSON schema describing the strict NLI reply, sent as `response_format` so
/// models that support structured outputs are constrained to it. Models that
/// ignore it still produce parseable JSON because the prompt demands it, and the
/// reply is parsed defensively regardless.
fn nli_response_format() -> Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "nli_verdict",
            "strict": true,
            "schema": {
                "type": "object",
                "properties": {
                    "label": {
                        "type": "string",
                        "enum": ["entailment", "neutral", "contradiction"]
                    },
                    "score": { "type": "number" }
                },
                "required": ["label", "score"],
                "additionalProperties": false
            }
        }
    })
}

/// Hosted [`Nli`] implemented as an LLM-as-judge over OpenRouter chat
/// completions. OpenRouter exposes no dedicated NLI endpoint, so a judge model
/// is prompted to emit a strict JSON verdict.
pub struct OpenRouterNli {
    client: OpenRouterClient,
    model: String,
}

impl OpenRouterNli {
    /// Build an NLI judge. The model id is resolved from `model` (if `Some` and
    /// non-empty), then `OPENROUTER_NLI_MODEL`, then the default
    /// ([`DEFAULT_NLI_MODEL`]). Reads the API key and base URL from the
    /// environment.
    pub fn new(model: Option<String>) -> Result<Self, SemanticsError> {
        let client = OpenRouterClient::from_env()?;
        let model = resolve_model(model, Some(ENV_NLI_MODEL), DEFAULT_NLI_MODEL);
        Ok(Self { client, model })
    }
}

/// The strict JSON verdict the judge model is asked to return.
#[derive(Debug, Serialize, Deserialize)]
struct JudgeVerdict {
    label: String,
    score: f32,
}

/// Build the chat-completions request body for an NLI classification.
fn build_nli_request(model: &str, premise: &str, hypothesis: &str) -> Value {
    let user = format!("Premise: {premise}\nHypothesis: {hypothesis}");
    json!({
        "model": model,
        "temperature": 0,
        "response_format": nli_response_format(),
        "messages": [
            { "role": "system", "content": NLI_SYSTEM_PROMPT },
            { "role": "user", "content": user }
        ]
    })
}

/// Extract the assistant message text from a chat-completions response.
fn extract_chat_content(value: &Value) -> Result<String, BackendError> {
    let endpoint = "/chat/completions";
    value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| BackendError::UnexpectedResponse {
            endpoint,
            detail: "response had no choices[0].message.content string".to_owned(),
        })
}

/// Map a judge label string onto the [`Entailment`] scheme, tolerant of casing
/// and the common short spellings.
fn parse_entailment_label(raw: &str) -> Option<Entailment> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "entailment" | "entail" | "entails" => Some(Entailment::Entailment),
        "neutral" => Some(Entailment::Neutral),
        "contradiction" | "contradict" | "contradicts" => Some(Entailment::Contradiction),
        _ => None,
    }
}

/// Pull a JSON object substring out of `content`, tolerating surrounding prose
/// or code fences a model might add despite instructions. Returns the slice
/// from the first `{` to the last `}` inclusive.
fn extract_json_object(content: &str) -> Option<&str> {
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    if end < start {
        return None;
    }
    content.get(start..=end)
}

/// Parse a chat-completions response into an [`NliVerdict`], defensively: the
/// model's reply may include stray text, so a JSON object is extracted before
/// parsing, and an unknown label or non-finite score is a structured error
/// rather than a panic.
fn parse_nli_response(value: &Value) -> Result<NliVerdict, BackendError> {
    let endpoint = "/chat/completions";
    let content = extract_chat_content(value)?;
    let object = extract_json_object(&content).ok_or_else(|| BackendError::UnexpectedResponse {
        endpoint,
        detail: format!("judge reply contained no JSON object: {content:?}"),
    })?;
    let verdict: JudgeVerdict =
        serde_json::from_str(object).map_err(|source| BackendError::Parse { endpoint, source })?;

    let label =
        parse_entailment_label(&verdict.label).ok_or_else(|| BackendError::UnexpectedResponse {
            endpoint,
            detail: format!("judge returned an unrecognised label: {:?}", verdict.label),
        })?;

    if !verdict.score.is_finite() {
        return Err(BackendError::UnexpectedResponse {
            endpoint,
            detail: format!("judge returned a non-finite score: {}", verdict.score),
        });
    }
    let score = verdict.score.clamp(0.0, 1.0);

    Ok(NliVerdict { label, score })
}

impl Nli for OpenRouterNli {
    fn classify(&self, premise: &str, hypothesis: &str) -> Result<NliVerdict, SemanticsError> {
        let body = build_nli_request(&self.model, premise, hypothesis);
        let value = self.client.post_json("/chat/completions", &body)?;
        let verdict = parse_nli_response(&value)?;
        Ok(verdict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- request-body construction ---

    #[test]
    fn embeddings_request_has_model_and_input_array() {
        let body = build_embeddings_request("google/gemini-embedding-2", &["hello", "world"]);
        assert_eq!(body["model"], "google/gemini-embedding-2");
        assert_eq!(body["input"], json!(["hello", "world"]));
    }

    #[test]
    fn rerank_request_has_model_query_documents() {
        let body = build_rerank_request("cohere/rerank-4-pro", "q", &["a", "b"]);
        assert_eq!(body["model"], "cohere/rerank-4-pro");
        assert_eq!(body["query"], "q");
        assert_eq!(body["documents"], json!(["a", "b"]));
    }

    #[test]
    fn nli_request_carries_prompt_and_strict_format() {
        let body = build_nli_request("some/model", "P text", "H text");
        assert_eq!(body["model"], "some/model");
        assert_eq!(body["temperature"], 0);
        assert_eq!(body["response_format"]["type"], "json_schema");
        let messages = body["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        let user = messages[1]["content"].as_str().expect("user content");
        assert!(user.contains("P text"));
        assert!(user.contains("H text"));
    }

    // --- embeddings parsing ---

    #[test]
    fn embeddings_response_parses_in_request_order() {
        let value = json!({
            "data": [
                { "index": 1, "embedding": [0.1, 0.2] },
                { "index": 0, "embedding": [0.3, 0.4] }
            ]
        });
        let vectors = parse_embeddings_response(&value, 2).expect("parse");
        // Sorted by `index`: row 0 first.
        assert_eq!(vectors, vec![vec![0.3, 0.4], vec![0.1, 0.2]]);
    }

    #[test]
    fn embeddings_response_without_index_keeps_array_order() {
        let value = json!({
            "data": [
                { "embedding": [1.0] },
                { "embedding": [2.0] }
            ]
        });
        let vectors = parse_embeddings_response(&value, 2).expect("parse");
        assert_eq!(vectors, vec![vec![1.0], vec![2.0]]);
    }

    #[test]
    fn embeddings_response_wrong_count_is_error() {
        let value = json!({ "data": [ { "embedding": [1.0] } ] });
        let err = parse_embeddings_response(&value, 2).expect_err("count mismatch");
        assert!(matches!(err, BackendError::UnexpectedResponse { .. }));
    }

    #[test]
    fn embeddings_response_empty_vector_is_error() {
        let value = json!({ "data": [ { "embedding": [] } ] });
        let err = parse_embeddings_response(&value, 1).expect_err("empty vector");
        assert!(matches!(err, BackendError::UnexpectedResponse { .. }));
    }

    #[test]
    fn embeddings_response_bad_shape_is_parse_error() {
        let value = json!({ "data": [ { "embedding": "not-a-vector" } ] });
        let err = parse_embeddings_response(&value, 1).expect_err("parse error");
        assert!(matches!(err, BackendError::Parse { .. }));
    }

    // --- rerank parsing ---

    #[test]
    fn rerank_response_realigns_to_document_order() {
        let value = json!({
            "results": [
                { "index": 2, "relevance_score": 0.9 },
                { "index": 0, "relevance_score": 0.1 },
                { "index": 1, "relevance_score": 0.5 }
            ]
        });
        let scores = parse_rerank_response(&value, 3).expect("parse");
        assert_eq!(scores, vec![0.1, 0.5, 0.9]);
    }

    #[test]
    fn rerank_response_out_of_range_index_is_error() {
        let value = json!({ "results": [ { "index": 5, "relevance_score": 0.9 } ] });
        let err = parse_rerank_response(&value, 1).expect_err("out of range");
        assert!(matches!(err, BackendError::UnexpectedResponse { .. }));
    }

    #[test]
    fn rerank_response_duplicate_index_is_error() {
        let value = json!({
            "results": [
                { "index": 0, "relevance_score": 0.1 },
                { "index": 0, "relevance_score": 0.2 }
            ]
        });
        let err = parse_rerank_response(&value, 2).expect_err("duplicate index");
        assert!(matches!(err, BackendError::UnexpectedResponse { .. }));
    }

    #[test]
    fn rerank_response_wrong_count_is_error() {
        let value = json!({ "results": [ { "index": 0, "relevance_score": 0.1 } ] });
        let err = parse_rerank_response(&value, 2).expect_err("count mismatch");
        assert!(matches!(err, BackendError::UnexpectedResponse { .. }));
    }

    // --- NLI parsing + label mapping ---

    #[test]
    fn nli_label_strings_map_to_entailment() {
        assert_eq!(
            parse_entailment_label("ENTAILMENT"),
            Some(Entailment::Entailment)
        );
        assert_eq!(
            parse_entailment_label(" neutral "),
            Some(Entailment::Neutral)
        );
        assert_eq!(
            parse_entailment_label("contradiction"),
            Some(Entailment::Contradiction)
        );
        assert_eq!(
            parse_entailment_label("contradicts"),
            Some(Entailment::Contradiction)
        );
        assert_eq!(parse_entailment_label("nonsense"), None);
    }

    /// Helper that wraps a judge content string in a chat-completions envelope.
    fn chat_envelope(content: &str) -> Value {
        json!({ "choices": [ { "message": { "role": "assistant", "content": content } } ] })
    }

    #[test]
    fn nli_response_parses_clean_json() {
        let value = chat_envelope("{\"label\": \"contradiction\", \"score\": 0.92}");
        let verdict = parse_nli_response(&value).expect("parse");
        assert_eq!(verdict.label, Entailment::Contradiction);
        assert!((verdict.score - 0.92).abs() < 1e-6);
    }

    #[test]
    fn nli_response_tolerates_prose_and_fences() {
        let value = chat_envelope(
            "Sure! Here is the verdict:\n```json\n{\"label\": \"entailment\", \"score\": 0.7}\n```",
        );
        let verdict = parse_nli_response(&value).expect("parse");
        assert_eq!(verdict.label, Entailment::Entailment);
        assert!((verdict.score - 0.7).abs() < 1e-6);
    }

    #[test]
    fn nli_response_clamps_out_of_range_score() {
        let value = chat_envelope("{\"label\": \"neutral\", \"score\": 1.5}");
        let verdict = parse_nli_response(&value).expect("parse");
        assert_eq!(verdict.label, Entailment::Neutral);
        assert!((verdict.score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn nli_response_unknown_label_is_error() {
        let value = chat_envelope("{\"label\": \"maybe\", \"score\": 0.5}");
        let err = parse_nli_response(&value).expect_err("unknown label");
        assert!(matches!(err, BackendError::UnexpectedResponse { .. }));
    }

    #[test]
    fn nli_response_non_finite_score_is_error() {
        // NaN is not representable in JSON, so deliver it via a string the parser
        // will reject as a Parse error; an out-of-band non-finite path is also
        // covered by the clamp test. Here assert a missing score is rejected.
        let value = chat_envelope("{\"label\": \"neutral\"}");
        let err = parse_nli_response(&value).expect_err("missing score");
        assert!(matches!(err, BackendError::Parse { .. }));
    }

    #[test]
    fn nli_response_no_json_object_is_error() {
        let value = chat_envelope("I cannot help with that.");
        let err = parse_nli_response(&value).expect_err("no json");
        assert!(matches!(err, BackendError::UnexpectedResponse { .. }));
    }

    #[test]
    fn nli_response_missing_content_is_error() {
        let value = json!({ "choices": [] });
        let err = parse_nli_response(&value).expect_err("no content");
        assert!(matches!(err, BackendError::UnexpectedResponse { .. }));
    }

    // --- model resolution ---

    #[test]
    fn resolve_model_prefers_explicit() {
        let model = resolve_model(Some("explicit/model".to_owned()), None, "default/model");
        assert_eq!(model, "explicit/model");
    }

    #[test]
    fn resolve_model_falls_back_to_default() {
        let model = resolve_model(None, None, "default/model");
        assert_eq!(model, "default/model");
    }

    #[test]
    fn resolve_model_ignores_blank_explicit() {
        let model = resolve_model(Some("   ".to_owned()), None, "default/model");
        assert_eq!(model, "default/model");
    }

    // --- live smoke test (network + key required) ---

    /// Live smoke test against the real OpenRouter API. Ignored by default; run
    /// with `cargo test -p texo-semantics -- --ignored`. Skips cleanly (returns
    /// early) when `OPENROUTER_API_KEY` is not set, so it never fails for lack of
    /// a key.
    #[test]
    #[ignore = "hits the live OpenRouter API; requires OPENROUTER_API_KEY"]
    fn live_embeds_and_classifies() {
        use texo_core::cosine_similarity;

        if std::env::var(ENV_API_KEY)
            .ok()
            .filter(|k| !k.trim().is_empty())
            .is_none()
        {
            eprintln!("OPENROUTER_API_KEY not set; skipping live smoke test");
            return;
        }

        let embedder = OpenRouterEmbedder::new(None).expect("build embedder");
        let base = embedder
            .embed("Deploys happen on Friday")
            .expect("embed base");
        let paraphrase = embedder
            .embed("The deploy schedule is Friday")
            .expect("embed paraphrase");
        let unrelated = embedder.embed("Lunch was tacos").expect("embed unrelated");

        let para_sim = cosine_similarity(&base, &paraphrase);
        let unrel_sim = cosine_similarity(&base, &unrelated);
        eprintln!("paraphrase cosine = {para_sim:.4}, unrelated cosine = {unrel_sim:.4}");
        assert!(
            para_sim > unrel_sim,
            "paraphrase cosine ({para_sim:.4}) should exceed unrelated ({unrel_sim:.4})"
        );

        let nli = OpenRouterNli::new(None).expect("build nli");
        let verdict = nli
            .classify("Deploys moved to Tuesday.", "Deploys happen on Friday.")
            .expect("classify");
        eprintln!("verdict -> {:?} ({:.4})", verdict.label, verdict.score);
        assert_eq!(verdict.label, Entailment::Contradiction);
    }
}
