//! Hosted semantic backends over the typed OpenAI-compatible gateway.
//!
//! These implement the texo semantic traits over HTTP using the crate's
//! synchronous OpenAI-compatible client. They run on any CPU and require only
//! an API key, which makes them the portable default backend.
//!
//! Three role adapters are provided:
//!
//! - [`OpenRouterEmbedder`] -> `POST /embeddings`
//! - [`OpenRouterRelater`] -> `POST /chat/completions`
//! - [`OpenRouterProposer`] -> `POST /chat/completions`
//!
//! Request-body construction and response parsing are factored into pure
//! functions ([`build_*_request`] / [`parse_*_response`]) so they can be unit
//! tested against hand-written JSON without any network access.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::gateway::{
    resolve_role, GatewayConfig, ModelRole, ResolvedRole, ResponseFormatPolicy, RoleOverrides,
};
use crate::semantics::{
    ClaimRelater, ClaimRelation, Embedder, ProposedClaim, Proposer, RelationVerdict, SemanticsError,
};
use crate::surfaces::openai::{ApiFailure, OpenAiCompatClient};

/// Failures from `OpenRouter` semantic response handling.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BackendError {
    /// A JSON request failed through the shared OpenAI-compatible client.
    #[error("semantics {provider}: {source}")]
    Http {
        /// Resolved provider id.
        provider: String,
        /// Typed transport cause.
        #[source]
        source: ApiFailure,
    },
    /// An `OpenRouter` response body could not be parsed as the expected JSON
    /// shape.
    #[error("could not parse model {endpoint} response")]
    Parse {
        /// The endpoint path whose response failed to parse.
        endpoint: &'static str,
        /// The typed JSON cause.
        #[source]
        source: serde_json::Error,
    },
    /// An `OpenRouter` response parsed as JSON but lacked required fields.
    #[error("model {endpoint} response was missing required data: {detail}")]
    UnexpectedResponse {
        /// The endpoint path whose response was malformed.
        endpoint: &'static str,
        /// Human-readable detail about what was missing or invalid.
        detail: String,
    },
    /// A model positively reported token-limit truncation.
    #[error("model {endpoint} response was truncated ({finish_reason}); max_tokens={max_tokens}")]
    Truncated {
        /// Endpoint path.
        endpoint: &'static str,
        /// Provider finish reason, preserved verbatim.
        finish_reason: String,
        /// Configured completion-token ceiling.
        max_tokens: u32,
    },
}

impl From<BackendError> for SemanticsError {
    fn from(err: BackendError) -> Self {
        SemanticsError::Backend {
            source: Box::new(err),
        }
    }
}

/// Version tag for the Stage-1 proposer prompt/output contract. Bump whenever
/// `PROPOSE_SYSTEM_PROMPT` or the parsed shape changes so the record-once cache
/// invalidates proposals produced by an older prompt.
const PROPOSE_PROMPT_VERSION: u32 = 3;
/// Version tag for the claim-relation prompt/output contract. Bump whenever
/// `RELATION_SYSTEM_PROMPT` or the parsed shape changes so the record-once cache
/// invalidates verdicts produced by an older prompt.
const RELATION_PROMPT_VERSION: u32 = 2;

/// A thin blocking HTTP client for the `OpenRouter` API.
///
/// Holds the shared OpenAI-compatible client. Shared by all backends.
struct OpenRouterClient {
    inner: OpenAiCompatClient,
    provider_id: String,
}

impl OpenRouterClient {
    fn from_role(role: &ResolvedRole) -> Result<Self, BackendError> {
        let inner = OpenAiCompatClient::from_role(role).map_err(|source| BackendError::Http {
            provider: role.provider_id.clone(),
            source,
        })?;
        Ok(Self {
            inner,
            provider_id: role.provider_id.clone(),
        })
    }

    /// POST `body` as JSON to `endpoint` (a path like `/embeddings`) and return
    /// the parsed JSON response.
    ///
    /// Retries, response status handling, and the shared deadline budget live in
    /// [`OpenAiCompatClient::post_json`].
    fn post_json(&self, endpoint: &'static str, body: &Value) -> Result<Value, BackendError> {
        self.inner
            .post_json(endpoint, body)
            .map_err(|source| BackendError::Http {
                provider: self.provider_id.clone(),
                source,
            })
    }
}

// =====================================================================
// Embeddings
// =====================================================================

/// Hosted [`Embedder`] backed by `OpenRouter`'s `/embeddings` endpoint.
pub struct OpenRouterEmbedder {
    client: OpenRouterClient,
    role: ResolvedRole,
}

impl OpenRouterEmbedder {
    /// Build an embedder through the shared gateway resolver.
    ///
    /// # Errors
    ///
    /// Returns [`SemanticsError::Backend`] when the resolved role has no key or
    /// the configured base URL is invalid.
    pub fn new(
        model: Option<String>,
        gateway: Option<&GatewayConfig>,
    ) -> Result<Self, SemanticsError> {
        let role = resolve_role(
            ModelRole::Embed,
            &RoleOverrides {
                model,
                ..RoleOverrides::default()
            },
            gateway,
        );
        let client = OpenRouterClient::from_role(&role)?;
        Ok(Self { client, role })
    }

    fn embed_chunk(
        &self,
        chunk: &[&str],
        vectors: &mut Vec<Vec<f32>>,
    ) -> Result<(), SemanticsError> {
        let body = build_embeddings_request(&self.role.config.model, chunk);
        match self.client.post_json("/embeddings", &body) {
            Ok(value) => {
                vectors.extend(parse_embeddings_response(&value, chunk.len())?);
                Ok(())
            }
            Err(error) if downshiftable_embedding_error(&error, chunk.len()) => {
                let split = chunk.len() / 2;
                self.embed_chunk(&chunk[..split], vectors)?;
                self.embed_chunk(&chunk[split..], vectors)
            }
            Err(error) => Err(error.into()),
        }
    }
}

fn downshiftable_embedding_error(error: &BackendError, chunk_len: usize) -> bool {
    if chunk_len <= 1 {
        return false;
    }
    matches!(
        error,
        BackendError::Http { source, .. }
            if source.status.is_some_and(|status| matches!(status, 400 | 413 | 422))
    )
}

/// Build the JSON request body for an embeddings call over `inputs`.
fn build_embeddings_request(model: &str, inputs: &[&str]) -> Value {
    json!({ "model": model, "input": inputs })
}

/// `OpenRouter` `/embeddings` response shape (the subset this backend reads).
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
        // Chunk to stay under the endpoint's per-request input cap; concatenate
        // in request order so the result still lines up 1:1 with `texts`.
        let mut vectors = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(self.role.profile.embed_batch_max.max(1)) {
            self.embed_chunk(chunk, &mut vectors)?;
        }
        Ok(vectors)
    }
}

// =====================================================================
/// Extract the assistant message text from a chat-completions response.
fn extract_chat_content(value: &Value, max_tokens: u32) -> Result<String, BackendError> {
    let endpoint = "/chat/completions";
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| BackendError::UnexpectedResponse {
            endpoint,
            detail: "response had no choices[0]; finish_reason=<absent>".to_string(),
        })?;
    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .or_else(|| choice.get("native_finish_reason").and_then(Value::as_str));
    let content = choice
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if finish_reason == Some("length") && (content.is_empty() || !content.contains('{')) {
        return Err(BackendError::Truncated {
            endpoint,
            finish_reason: "length".to_string(),
            max_tokens,
        });
    }
    if content.is_empty() {
        return Err(BackendError::UnexpectedResponse {
            endpoint,
            detail: format!(
                "response had no choices[0].message.content string; finish_reason={}",
                finish_reason.unwrap_or("<absent>")
            ),
        });
    }
    Ok(content.to_string())
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
- older \"The all-hands moved to Monday.\" / newer \"The all-hands moved to \
Thursday.\" -> supersedes (a later move of the SAME thing replaces the earlier \
move; the most recent change wins — this is NOT a conflict).\n\
- older \"The cache TTL is 60 seconds.\" / newer \"The cache TTL is 300 \
seconds.\" -> conflict (a different value, but no wording signals an update).\n\
- older \"Dana leads the on-call rotation.\" / newer \"Raj is no longer on the \
on-call rotation.\" -> unrelated (a claim that ONE person left a role does not \
supersede a claim that a DIFFERENT person holds a role — the subjects differ; a \
negative/consequence claim about entity A never supersedes a positive claim \
about entity B).\n\
- older \"The platform stores events in BatPak.\" / newer \"BatPak keeps each \
event as a content-addressed log entry.\" -> unrelated (the second ELABORATES a \
detail of the same system; it does not state a different value for the first's \
attribute, so it does not supersede it).\n\
- older \"Backups run nightly.\" / newer \"The staging cluster has 3 nodes.\" -> \
unrelated (different subjects).\n\
- older \"The service exposes a REST API.\" / newer \"The service emits metrics \
to Prometheus.\" -> unrelated (two DIFFERENT facts about the same system, each \
still true — different attributes, so neither replaces the other).\n\
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

/// Hosted [`ClaimRelater`] implemented as an LLM-as-judge over `OpenRouter` chat
/// completions. This is the primary relating backend; it makes the single richer
/// judgment (shared subject? update or conflict?) that embeddings + 3-way NLI
/// could not.
pub struct OpenRouterRelater {
    client: OpenRouterClient,
    role: ResolvedRole,
}

impl OpenRouterRelater {
    /// Build a relation judge through the shared gateway resolver.
    ///
    /// # Errors
    ///
    /// Returns [`SemanticsError::Backend`] when the resolved role has no key or
    /// the configured base URL is invalid.
    pub fn new(
        model: Option<String>,
        gateway: Option<&GatewayConfig>,
    ) -> Result<Self, SemanticsError> {
        let role = resolve_role(
            ModelRole::Relate,
            &RoleOverrides {
                model,
                ..RoleOverrides::default()
            },
            gateway,
        );
        let client = OpenRouterClient::from_role(&role)?;
        Ok(Self { client, role })
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
fn build_relation_request(role: &ResolvedRole, older: &str, newer: &str) -> Value {
    let user = format!("First (older) claim: {older}\nSecond (newer) claim: {newer}");
    let mut body = json!({
        "model": role.config.model,
        "temperature": role.config.temperature,
        "max_tokens": role.config.max_completion_tokens,
        "messages": [
            { "role": "system", "content": RELATION_SYSTEM_PROMPT },
            { "role": "user", "content": user }
        ]
    });
    if matches!(
        role.config.response_format,
        ResponseFormatPolicy::JsonSchema
    ) && role.profile.strict_json_schema_ok
    {
        body["response_format"] = relation_response_format();
    }
    body
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
fn parse_relation_response(
    value: &Value,
    max_tokens: u32,
) -> Result<RelationVerdict, BackendError> {
    let endpoint = "/chat/completions";
    let content = extract_chat_content(value, max_tokens)?;
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
        let body = build_relation_request(&self.role, older, newer);
        let value = self.client.post_json("/chat/completions", &body)?;
        let verdict = parse_relation_response(&value, self.role.config.max_completion_tokens)?;
        Ok(verdict)
    }

    fn fingerprint(&self) -> String {
        format!(
            "{}:{}|relation-v{RELATION_PROMPT_VERSION}",
            self.role.provider_id, self.role.config.model
        )
    }
}

// =====================================================================
// Claim proposal (Stage 1 extraction over chat/completions)
// =====================================================================

/// System prompt for the Stage-1 proposer. It must extract *atomic* claims and
/// copy values verbatim — the deterministic faithfulness gate downstream rejects
/// anything ungrounded, so faithfulness here is cheaper than a later rejection.
const PROPOSE_SYSTEM_PROMPT: &str = "You extract DURABLE claims from one span of \
a team's engineering documentation. A durable claim states a fact about how the \
system or team CURRENTLY operates and that a teammate would still need to know \
months later: a decision, an owner, a schedule, a configuration value, an \
architectural fact, or a policy. Each claim is atomic — ONE subject, one \
predicate, one value — and copies entities, names, numbers, dates, and values \
EXACTLY as they appear (never infer, generalize, or add). Preserve update \
wording such as \"now\", \"moved to\", or \"no longer\" when the span uses it.\n\
IMPORTANT: when a sentence mixes a durable fact with narrative or a reason, \
extract the durable fact and DROP the narrative — do not skip the sentence. \
E.g. \"Deploys moved to Tuesday after we realized Wednesday collided with the \
all-hands\" yields the claim \"Deploys moved to Tuesday.\"\n\
Do NOT extract (return fewer, better claims):\n\
- transient status or progress (\"the migration is 60% done\", \"dual-write is \
done\");\n\
- pure incident color with no decision in it (\"the rotation revolted\", \"a \
Slack thread ensued\");\n\
- opinions or judgments (\"Alice is a bottleneck\");\n\
- low-level mechanics that merely ELABORATE a higher-level fact — prefer the \
single headline statement. If the span says the platform now uses BatPak and \
also describes the table-level mechanics of that migration, extract only the \
headline (\"uses BatPak now\"), not the mechanics.\n\
Skip questions, tasks, and greetings. If the span states no durable claim, \
return an empty list. For each claim provide: text (one faithful declarative \
sentence), subject, predicate, object, and confidence (an integer from 0 to \
100). Respond with ONLY a JSON object of the form {\"claims\": [{\"text\": ..., \
\"subject\": ..., \"predicate\": ..., \"object\": ..., \"confidence\": ...}]}. \
No prose, no markdown, no code fences.";

/// Hosted Stage-1 claim proposer over `OpenRouter` chat completions. Implements
/// [`Proposer`].
pub struct OpenRouterProposer {
    client: OpenRouterClient,
    role: ResolvedRole,
}

impl OpenRouterProposer {
    /// Build a proposer through the shared gateway resolver.
    ///
    /// # Errors
    ///
    /// Returns [`SemanticsError::Backend`] when the resolved role has no key or
    /// the configured base URL is invalid.
    pub fn new(
        model: Option<String>,
        gateway: Option<&GatewayConfig>,
    ) -> Result<Self, SemanticsError> {
        let role = resolve_role(
            ModelRole::Propose,
            &RoleOverrides {
                model,
                ..RoleOverrides::default()
            },
            gateway,
        );
        let client = OpenRouterClient::from_role(&role)?;
        Ok(Self { client, role })
    }
}

impl Proposer for OpenRouterProposer {
    fn propose(
        &self,
        span_text: &str,
        heading_path: &[String],
    ) -> Result<Vec<ProposedClaim>, SemanticsError> {
        let body = build_propose_request(&self.role, span_text, heading_path);
        let value = self.client.post_json("/chat/completions", &body)?;
        let claims = parse_propose_response(&value, self.role.config.max_completion_tokens)?;
        Ok(claims)
    }

    fn fingerprint(&self) -> String {
        format!(
            "{}:{}|propose-v{PROPOSE_PROMPT_VERSION}",
            self.role.provider_id, self.role.config.model
        )
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
fn build_propose_request(role: &ResolvedRole, span_text: &str, heading_path: &[String]) -> Value {
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
        "model": role.config.model,
        "temperature": role.config.temperature,
        "max_tokens": role.config.max_completion_tokens,
        "messages": [
            { "role": "system", "content": PROPOSE_SYSTEM_PROMPT },
            { "role": "user", "content": user }
        ]
    })
}

/// Parse a chat-completions response into proposed claims, defensively. Claims
/// whose `text` is blank are dropped; confidence is clamped to `[0, 1]` and
/// rescaled to parts-per-million.
fn parse_propose_response(
    value: &Value,
    max_tokens: u32,
) -> Result<Vec<ProposedClaim>, BackendError> {
    let endpoint = "/chat/completions";
    let content = extract_chat_content(value, max_tokens)?;
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
mod tests;
