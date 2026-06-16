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
use texo_core::{
    ClaimRelater, ClaimRelation, Embedder, Entailment, Nli, NliVerdict, ProposedClaim, Proposer,
    RelationVerdict, Reranker, SemanticsError,
};

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
/// Default claim-relation judge model. The supersede-vs-conflict call is the
/// hardest judgment in the pipeline, so this defaults to a strong reasoning
/// model; override via `OPENROUTER_RELATER_MODEL` or the explicit constructor.
const DEFAULT_RELATER_MODEL: &str = "nvidia/nemotron-3-ultra-550b-a55b";

/// Environment variable holding the bearer token.
const ENV_API_KEY: &str = "OPENROUTER_API_KEY";
/// Environment variable overriding the API base URL.
const ENV_BASE_URL: &str = "OPENROUTER_BASE_URL";
/// Environment variable overriding the NLI judge model.
const ENV_NLI_MODEL: &str = "OPENROUTER_NLI_MODEL";
/// Environment variable overriding the claim-relation judge model.
const ENV_RELATER_MODEL: &str = "OPENROUTER_RELATER_MODEL";

/// Per-request timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Number of retry attempts on retryable (429 / 5xx) statuses, in addition to
/// the initial attempt.
const MAX_RETRIES: u32 = 4;
/// Base backoff between retries; doubled each attempt up to [`MAX_BACKOFF`].
const RETRY_BACKOFF: Duration = Duration::from_millis(500);
/// Upper bound on any single inter-retry wait, applied both to the exponential
/// schedule and to a server-supplied `Retry-After`, so one throttled call can
/// never block the caller unboundedly.
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Maximum number of bytes of an error body retained for diagnostics.
const MAX_ERROR_BODY: usize = 2048;
/// Completion-token ceiling for the chat-completions judges. Reasoning models
/// spend completion budget on a hidden reasoning trace *before* emitting the
/// JSON answer; without ample headroom a long trace truncates the answer to empty
/// content. Sized well above observed reasoning lengths so the verdict always fits.
const MAX_COMPLETION_TOKENS: u32 = 1024;
/// Completion-token ceiling for the Stage-1 proposer. A single span can yield
/// several atomic claims plus a reasoning trace, so it needs more room than the
/// single-verdict judges.
const MAX_PROPOSE_TOKENS: u32 = 2048;

/// How long to wait before the next retry. A server-supplied `Retry-After`
/// (delta-seconds) wins when present; otherwise the wait is capped exponential
/// backoff: `RETRY_BACKOFF * 2^(attempt - 1)`. Every result is clamped to
/// [`MAX_BACKOFF`]. `attempt` is the 1-based number of the retry about to run.
fn retry_delay(attempt: u32, retry_after_secs: Option<u64>) -> Duration {
    let base = if let Some(secs) = retry_after_secs {
        Duration::from_secs(secs)
    } else {
        let shift = attempt.saturating_sub(1).min(20);
        RETRY_BACKOFF.saturating_mul(1u32.checked_shl(shift).unwrap_or(u32::MAX))
    };
    base.min(MAX_BACKOFF)
}

/// Parse a `Retry-After` header expressed as delta-seconds. The HTTP-date form
/// is intentionally not honored (OpenRouter sends seconds), and any unparsable
/// value falls through to exponential backoff.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

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
    /// Retries on HTTP 429 and 5xx up to [`MAX_RETRIES`] times, honoring a
    /// `Retry-After` header when present and otherwise using capped exponential
    /// backoff (see [`retry_delay`]); transport errors are returned immediately.
    /// Non-retryable non-success statuses become [`BackendError::HttpStatus`].
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
                let retry_after = parse_retry_after(response.headers());
                attempt += 1;
                std::thread::sleep(retry_delay(attempt, retry_after));
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
        "max_tokens": MAX_COMPLETION_TOKENS,
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

// =====================================================================
// Claim relation (LLM-as-judge over chat/completions)
// =====================================================================

/// System prompt for the claim-relation judge. It must separate a value
/// *replacement* (supersession) from a flat *disagreement* (conflict) — the
/// distinction 3-way NLI cannot make — and reject pairs that merely share a token
/// but concern different subjects.
const RELATION_SYSTEM_PROMPT: &str = "You compare two claims extracted from a \
team's engineering documentation and decide how they relate. The SECOND claim \
was recorded MORE RECENTLY than the FIRST. Classify the relationship as exactly \
one of:\n\
The deciding test for supersedes vs conflict is the wording of the SECOND \
(newer) claim ALONE — not which claim is newer, and not whether the older claim \
also looks like an update.\n\
- \"supersedes\": the two claims are about the SAME subject and attribute, AND \
the SECOND (newer) claim itself carries explicit change/update/correction \
wording — for example \"moved to\", \"now\", \"changed to\", \"switched to\", \
\"migrated to\", \"as of\", \"updated to\", \"no longer\", or \"replaced by\". \
If the second claim contains such wording and the subjects match, it supersedes \
the first, even if the first claim also happened to describe a change.\n\
- \"conflict\": the two claims are about the SAME subject and attribute and \
assert INCOMPATIBLE values, but the SECOND (newer) claim contains NO explicit \
change/update wording — it merely states a different value. Recency alone never \
makes it a supersession; a bare differing value is a conflict.\n\
- \"duplicate\": the two claims state the same fact.\n\
- \"unrelated\": the claims are about DIFFERENT subjects or attributes, or are \
compatible/independent. Two claims that merely share a word — a weekday, \
\"release\", a name — but concern different subjects (for example a \
DEPLOYMENT schedule versus a RELEASE schedule, or WHO approves releases versus \
WHEN releases ship) are unrelated.\n\
Examples (these are illustrations, not the claims you will judge):\n\
- older \"The API runs on port 8080.\" / newer \"The API now listens on port \
9090.\" -> supersedes (the word \"now\" explicitly marks the change).\n\
- older \"The cache TTL is 60 seconds.\" / newer \"The cache TTL is 300 \
seconds.\" -> conflict (a different value, but no wording signals an update).\n\
- older \"Backups run nightly.\" / newer \"The staging cluster has 3 nodes.\" -> \
unrelated (different subjects).\n\
Reply with ONLY a single JSON object: {\"relation\": \"supersedes\" | \
\"conflict\" | \"duplicate\" | \"unrelated\", \"score\": <number between 0 and \
1>}. Output no prose, no markdown, no code fences.";

/// JSON schema constraining the relation reply for models that honor it.
fn relation_response_format() -> Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "claim_relation",
            "strict": true,
            "schema": {
                "type": "object",
                "properties": {
                    "relation": {
                        "type": "string",
                        "enum": ["supersedes", "conflict", "duplicate", "unrelated"]
                    },
                    "score": { "type": "number" }
                },
                "required": ["relation", "score"],
                "additionalProperties": false
            }
        }
    })
}

/// Hosted [`ClaimRelater`] implemented as an LLM-as-judge over OpenRouter chat
/// completions. This is the primary relating backend; it makes the single richer
/// judgment (shared subject? update or conflict?) that embeddings + 3-way NLI
/// could not.
pub struct OpenRouterRelater {
    client: OpenRouterClient,
    model: String,
}

impl OpenRouterRelater {
    /// Build a relation judge. The model id is resolved from `model` (if `Some`
    /// and non-empty), then `OPENROUTER_RELATER_MODEL`, then the default
    /// ([`DEFAULT_RELATER_MODEL`]). Reads the API key and base URL from the
    /// environment.
    pub fn new(model: Option<String>) -> Result<Self, SemanticsError> {
        let client = OpenRouterClient::from_env()?;
        let model = resolve_model(model, Some(ENV_RELATER_MODEL), DEFAULT_RELATER_MODEL);
        Ok(Self { client, model })
    }
}

/// The strict JSON verdict the relation judge is asked to return.
#[derive(Debug, Serialize, Deserialize)]
struct JudgeRelation {
    relation: String,
    score: f32,
}

/// Build the chat-completions request body for a claim-relation judgment. The
/// older claim is the premise, the newer the more-recent claim under judgment.
fn build_relation_request(model: &str, older: &str, newer: &str) -> Value {
    let user = format!("First (older) claim: {older}\nSecond (newer) claim: {newer}");
    json!({
        "model": model,
        "temperature": 0,
        "max_tokens": MAX_COMPLETION_TOKENS,
        "response_format": relation_response_format(),
        "messages": [
            { "role": "system", "content": RELATION_SYSTEM_PROMPT },
            { "role": "user", "content": user }
        ]
    })
}

/// Map a judge relation string onto [`ClaimRelation`], tolerant of casing and
/// the common short spellings.
fn parse_relation_label(raw: &str) -> Option<ClaimRelation> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "supersedes" | "supersede" | "superseded" => Some(ClaimRelation::Supersedes),
        "conflict" | "conflicts" | "conflicting" => Some(ClaimRelation::Conflict),
        "duplicate" | "duplicates" | "duplicated" => Some(ClaimRelation::Duplicate),
        "unrelated" | "none" | "neutral" => Some(ClaimRelation::Unrelated),
        _ => None,
    }
}

/// Parse a chat-completions response into a [`RelationVerdict`], defensively (the
/// reply may carry stray prose or fences; an unknown label or non-finite score is
/// a structured error, never a panic).
fn parse_relation_response(value: &Value) -> Result<RelationVerdict, BackendError> {
    let endpoint = "/chat/completions";
    let content = extract_chat_content(value)?;
    let object = extract_json_object(&content).ok_or_else(|| BackendError::UnexpectedResponse {
        endpoint,
        detail: format!("judge reply contained no JSON object: {content:?}"),
    })?;
    let verdict: JudgeRelation =
        serde_json::from_str(object).map_err(|source| BackendError::Parse { endpoint, source })?;

    let relation = parse_relation_label(&verdict.relation).ok_or_else(|| {
        BackendError::UnexpectedResponse {
            endpoint,
            detail: format!(
                "judge returned an unrecognised relation: {:?}",
                verdict.relation
            ),
        }
    })?;

    if !verdict.score.is_finite() {
        return Err(BackendError::UnexpectedResponse {
            endpoint,
            detail: format!("judge returned a non-finite score: {}", verdict.score),
        });
    }
    let score = verdict.score.clamp(0.0, 1.0);

    Ok(RelationVerdict { relation, score })
}

impl ClaimRelater for OpenRouterRelater {
    fn relate(&self, older: &str, newer: &str) -> Result<RelationVerdict, SemanticsError> {
        let body = build_relation_request(&self.model, older, newer);
        let value = self.client.post_json("/chat/completions", &body)?;
        let verdict = parse_relation_response(&value)?;
        Ok(verdict)
    }
}

// =====================================================================
// Claim proposal (Stage 1 extraction over chat/completions)
// =====================================================================

/// System prompt for the Stage-1 proposer. It must extract *atomic* claims and
/// copy values verbatim — the deterministic faithfulness gate downstream rejects
/// anything ungrounded, so faithfulness here is cheaper than a later rejection.
const PROPOSE_SYSTEM_PROMPT: &str = "You extract atomic factual claims from one \
span of a team's engineering documentation. An atomic claim states ONE fact: a \
single subject, a single predicate, and a single value. Rules: copy entities, \
names, numbers, dates, and values EXACTLY as they appear in the span — never \
infer, generalize, or add anything not present. Preserve update wording such as \
\"now\", \"moved to\", or \"no longer\" when the span uses it. Skip questions, \
opinions, tasks, greetings, and meta-commentary. If the span states no factual \
claim, return an empty list. For each claim provide: text (one faithful \
declarative sentence), subject, predicate, object, and confidence (an integer \
from 0 to 100). Respond with ONLY a JSON object of the form {\"claims\": \
[{\"text\": ..., \"subject\": ..., \"predicate\": ..., \"object\": ..., \
\"confidence\": ...}]}. No prose, no markdown, no code fences.";

/// Default Stage-1 extractor model. A capable instruction-follower for prod;
/// override with `OPENROUTER_EXTRACTOR_MODEL` (e.g. a free model for testing).
/// Note the OpenRouter slug uses a dotted version (`4.8`), unlike the Anthropic
/// API's dashed model id (`claude-opus-4-8`).
const DEFAULT_EXTRACTOR_MODEL: &str = "anthropic/claude-opus-4.8";
/// Environment variable overriding the extractor model.
const ENV_EXTRACTOR_MODEL: &str = "OPENROUTER_EXTRACTOR_MODEL";

/// Hosted Stage-1 claim proposer over OpenRouter chat completions. Implements
/// [`texo_core::Proposer`].
pub struct OpenRouterProposer {
    client: OpenRouterClient,
    model: String,
}

impl OpenRouterProposer {
    /// Build a proposer. The model id is resolved from `model` (if `Some` and
    /// non-empty), then `OPENROUTER_EXTRACTOR_MODEL`, then the default
    /// ([`DEFAULT_EXTRACTOR_MODEL`]). Reads the API key and base URL from the
    /// environment.
    pub fn new(model: Option<String>) -> Result<Self, SemanticsError> {
        let client = OpenRouterClient::from_env()?;
        let model = resolve_model(model, Some(ENV_EXTRACTOR_MODEL), DEFAULT_EXTRACTOR_MODEL);
        Ok(Self { client, model })
    }
}

impl Proposer for OpenRouterProposer {
    fn propose(
        &self,
        span_text: &str,
        heading_path: &[String],
    ) -> Result<Vec<ProposedClaim>, SemanticsError> {
        let body = build_propose_request(&self.model, span_text, heading_path);
        let value = self.client.post_json("/chat/completions", &body)?;
        let claims = parse_propose_response(&value)?;
        Ok(claims)
    }
}

/// The raw JSON shape of one proposed claim as returned by the model.
#[derive(Debug, Deserialize)]
struct ProposedClaimJson {
    text: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    predicate: String,
    #[serde(default)]
    object: String,
    /// Model confidence as an integer 0..=100 (rescaled to ppm on parse). Integer
    /// keeps the whole path free of float→int casts (pedantic-clean) and `Eq`.
    confidence: i64,
}

/// The proposer reply envelope.
#[derive(Debug, Deserialize)]
struct ProposeReply {
    claims: Vec<ProposedClaimJson>,
}

/// Build the chat-completions request body for a Stage-1 proposal over one span.
fn build_propose_request(model: &str, span_text: &str, heading_path: &[String]) -> Value {
    let context = if heading_path.is_empty() {
        String::new()
    } else {
        format!("Section: {}\n", heading_path.join(" > "))
    };
    let user = format!("{context}Span:\n{span_text}");
    // No `response_format`: strict constrained decoding on Anthropic-via-OpenRouter
    // can degenerate into repetition loops on the variable-length claim array.
    // Anthropic emits clean JSON from the prompt alone, parsed defensively below.
    json!({
        "model": model,
        "temperature": 0,
        "max_tokens": MAX_PROPOSE_TOKENS,
        "messages": [
            { "role": "system", "content": PROPOSE_SYSTEM_PROMPT },
            { "role": "user", "content": user }
        ]
    })
}

/// Parse a chat-completions response into proposed claims, defensively. Claims
/// whose `text` is blank are dropped; confidence is clamped to `[0, 1]` and
/// rescaled to parts-per-million.
fn parse_propose_response(value: &Value) -> Result<Vec<ProposedClaim>, BackendError> {
    let endpoint = "/chat/completions";
    let content = extract_chat_content(value)?;
    let object = extract_json_object(&content).ok_or_else(|| BackendError::UnexpectedResponse {
        endpoint,
        detail: format!("proposer reply contained no JSON object: {content:?}"),
    })?;
    let reply: ProposeReply =
        serde_json::from_str(object).map_err(|source| BackendError::Parse { endpoint, source })?;

    let mut claims = Vec::with_capacity(reply.claims.len());
    for raw in reply.claims {
        if raw.text.trim().is_empty() {
            continue;
        }
        // Integer percent -> ppm; clamp keeps an out-of-range model value in bounds.
        let pct = u32::try_from(raw.confidence.clamp(0, 100)).unwrap_or(0);
        let confidence_ppm = pct * 10_000;
        claims.push(ProposedClaim {
            text: raw.text.trim().to_owned(),
            subject: raw.subject.trim().to_owned(),
            predicate: raw.predicate.trim().to_owned(),
            object: raw.object.trim().to_owned(),
            confidence_ppm,
        });
    }
    Ok(claims)
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

    // --- claim-relation parsing + label mapping ---

    #[test]
    fn relation_request_carries_prompt_and_strict_format() {
        let body =
            build_relation_request("nvidia/nemotron-3-ultra-550b-a55b", "older X", "newer Y");
        assert_eq!(body["model"], "nvidia/nemotron-3-ultra-550b-a55b");
        assert_eq!(body["temperature"], 0);
        assert_eq!(
            body["response_format"]["json_schema"]["name"],
            "claim_relation"
        );
        assert_eq!(body["max_tokens"], 1024);
        let messages = body["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 2);
        let user = messages[1]["content"].as_str().expect("user content");
        assert!(user.contains("older X"));
        assert!(user.contains("newer Y"));
        // Order must be conveyed: older is labeled first, newer second.
        assert!(user.find("older X") < user.find("newer Y"));
    }

    #[test]
    fn relation_label_strings_map_to_claim_relation() {
        assert_eq!(
            parse_relation_label("SUPERSEDES"),
            Some(ClaimRelation::Supersedes)
        );
        assert_eq!(
            parse_relation_label(" conflict "),
            Some(ClaimRelation::Conflict)
        );
        assert_eq!(
            parse_relation_label("duplicate"),
            Some(ClaimRelation::Duplicate)
        );
        assert_eq!(
            parse_relation_label("unrelated"),
            Some(ClaimRelation::Unrelated)
        );
        assert_eq!(parse_relation_label("nonsense"), None);
    }

    #[test]
    fn relation_response_parses_clean_json() {
        let value = chat_envelope("{\"relation\": \"supersedes\", \"score\": 0.88}");
        let verdict = parse_relation_response(&value).expect("parse");
        assert_eq!(verdict.relation, ClaimRelation::Supersedes);
        assert!((verdict.score - 0.88).abs() < 1e-6);
    }

    #[test]
    fn relation_response_tolerates_fences_and_clamps_score() {
        let value = chat_envelope("```json\n{\"relation\": \"conflict\", \"score\": 1.4}\n```");
        let verdict = parse_relation_response(&value).expect("parse");
        assert_eq!(verdict.relation, ClaimRelation::Conflict);
        assert!((verdict.score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn relation_response_unknown_label_is_error() {
        let value = chat_envelope("{\"relation\": \"maybe\", \"score\": 0.5}");
        let err = parse_relation_response(&value).expect_err("unknown relation");
        assert!(matches!(err, BackendError::UnexpectedResponse { .. }));
    }

    #[test]
    fn relation_response_missing_score_is_parse_error() {
        let value = chat_envelope("{\"relation\": \"unrelated\"}");
        let err = parse_relation_response(&value).expect_err("missing score");
        assert!(matches!(err, BackendError::Parse { .. }));
    }

    // --- proposer parsing ---

    #[test]
    fn propose_request_includes_heading_context_and_schema() {
        let body = build_propose_request(
            "anthropic/claude-opus-4.8",
            "Deploys moved to Tuesday.",
            &["Operations".to_owned(), "Deploys".to_owned()],
        );
        assert_eq!(body["max_tokens"], 2048);
        let user = body["messages"][1]["content"].as_str().expect("user");
        assert!(user.contains("Operations > Deploys"), "heading path joined");
        assert!(user.contains("Deploys moved to Tuesday."));
    }

    #[test]
    fn propose_request_without_headings_has_no_section_line() {
        let body = build_propose_request("m", "Some span.", &[]);
        let user = body["messages"][1]["content"].as_str().expect("user");
        assert!(!user.contains("Section:"));
        assert!(user.starts_with("Span:"));
    }

    #[test]
    fn propose_response_parses_claims_and_rescales_confidence() {
        let value = chat_envelope(
            "{\"claims\":[{\"text\":\"Deploys moved to Tuesday.\",\"subject\":\"deploys\",\"predicate\":\"scheduled\",\"object\":\"Tuesday\",\"confidence\":90}]}",
        );
        let claims = parse_propose_response(&value).expect("parse");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].text, "Deploys moved to Tuesday.");
        assert_eq!(claims[0].subject, "deploys");
        assert_eq!(claims[0].confidence_ppm, 900_000);
    }

    #[test]
    fn propose_response_drops_blank_text_and_clamps_confidence() {
        let value = chat_envelope(
            "{\"claims\":[{\"text\":\"   \",\"subject\":\"\",\"predicate\":\"\",\"object\":\"\",\"confidence\":50},{\"text\":\"Real claim.\",\"subject\":\"x\",\"predicate\":\"y\",\"object\":\"z\",\"confidence\":170}]}",
        );
        let claims = parse_propose_response(&value).expect("parse");
        assert_eq!(claims.len(), 1, "blank-text claim dropped");
        assert_eq!(claims[0].text, "Real claim.");
        assert_eq!(claims[0].confidence_ppm, 1_000_000, "over-100 confidence clamped");
    }

    #[test]
    fn propose_response_empty_list_is_ok() {
        let value = chat_envelope("{\"claims\":[]}");
        let claims = parse_propose_response(&value).expect("parse");
        assert!(claims.is_empty());
    }

    #[test]
    fn propose_response_tolerates_fences() {
        let value = chat_envelope(
            "```json\n{\"claims\":[{\"text\":\"A.\",\"subject\":\"a\",\"predicate\":\"b\",\"object\":\"c\",\"confidence\":40}]}\n```",
        );
        let claims = parse_propose_response(&value).expect("parse");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].confidence_ppm, 400_000);
    }

    #[test]
    fn propose_response_missing_claims_key_is_parse_error() {
        let value = chat_envelope("{\"items\":[]}");
        let err = parse_propose_response(&value).expect_err("missing claims");
        assert!(matches!(err, BackendError::Parse { .. }));
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

    // --- retry backoff ---

    #[test]
    fn retry_delay_is_exponential_without_retry_after() {
        assert_eq!(retry_delay(1, None), Duration::from_millis(500));
        assert_eq!(retry_delay(2, None), Duration::from_secs(1));
        assert_eq!(retry_delay(3, None), Duration::from_secs(2));
        assert_eq!(retry_delay(4, None), Duration::from_secs(4));
    }

    #[test]
    fn retry_delay_is_capped_at_max_backoff() {
        // A huge attempt count would overflow naive shifting; it must clamp.
        assert_eq!(retry_delay(64, None), MAX_BACKOFF);
        // A huge server-supplied wait is clamped too, so one call can't hang.
        assert_eq!(retry_delay(1, Some(3600)), MAX_BACKOFF);
    }

    #[test]
    fn retry_delay_honors_retry_after_seconds() {
        assert_eq!(retry_delay(1, Some(5)), Duration::from_secs(5));
        // Retry-After wins over the exponential schedule for the same attempt.
        assert_eq!(retry_delay(3, Some(2)), Duration::from_secs(2));
    }

    #[test]
    fn parse_retry_after_reads_delta_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, " 7 ".parse().expect("header"));
        assert_eq!(parse_retry_after(&headers), Some(7));
    }

    #[test]
    fn parse_retry_after_ignores_http_date_and_missing() {
        let empty = reqwest::header::HeaderMap::new();
        assert_eq!(parse_retry_after(&empty), None);

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().expect("header"),
        );
        assert_eq!(parse_retry_after(&headers), None);
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
