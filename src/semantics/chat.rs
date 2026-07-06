//! Chat-completions plumbing.
//!
//! The memory-grounded system prompt and request body are built by pure
//! functions (unit-tested, no network). Only [`complete`] talks HTTP, against
//! any OpenAI-compatible `/chat/completions` endpoint using the same environment
//! conventions as the semantic backends (`OPENROUTER_BASE_URL`,
//! `OPENROUTER_API_KEY`) plus `OPENROUTER_CHAT_MODEL` for the chat role.

use std::fmt::Write as _;

use serde_json::{json, Value};

use crate::error::TexoError;
use crate::surfaces::openai::OpenAiCompatClient;

/// Default chat model (an `OpenRouter` slug, like the other role defaults).
/// Override with `OPENROUTER_CHAT_MODEL` — e.g. `qwen3.7-max` when
/// `OPENROUTER_BASE_URL` points at Qwen Cloud's `DashScope` compatible mode.
pub const DEFAULT_CHAT_MODEL: &str = "anthropic/claude-opus-4.8";
/// Default OpenAI-compatible base URL.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
/// Environment variable holding the bearer token.
pub const ENV_API_KEY: &str = "OPENROUTER_API_KEY";
/// Environment variable overriding the API base URL.
pub const ENV_BASE_URL: &str = "OPENROUTER_BASE_URL";
/// Environment variable overriding the chat model.
pub const ENV_CHAT_MODEL: &str = "OPENROUTER_CHAT_MODEL";
/// Completion-token ceiling for one assistant reply.
const MAX_REPLY_TOKENS: u32 = 1024;

/// Resolved chat backend configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatConfig {
    /// OpenAI-compatible API base URL.
    pub base_url: String,
    /// Bearer token.
    pub api_key: String,
    /// Chat model id.
    pub model: String,
}

impl ChatConfig {
    /// Resolve from explicit values. `None` when no API key is set.
    #[must_use]
    pub fn from_env_vars(
        api_key: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
    ) -> Option<Self> {
        let api_key = api_key.filter(|key| !key.trim().is_empty())?;
        let base_url = base_url
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned())
            .trim_end_matches('/')
            .to_string();
        let model = model
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CHAT_MODEL.to_owned());
        Some(Self {
            base_url,
            api_key,
            model,
        })
    }

    /// Resolve from the environment. `None` when no API key is set; chat is
    /// disabled but memory endpoints can keep working.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        Self::from_env_vars(
            std::env::var(ENV_API_KEY).ok(),
            std::env::var(ENV_BASE_URL).ok(),
            std::env::var(ENV_CHAT_MODEL).ok(),
        )
    }
}

/// Speaker role in a chat transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    /// User-authored turn.
    User,
    /// Assistant-authored turn.
    Assistant,
}

impl Speaker {
    fn role(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// One prior chat turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Utterance {
    /// Speaker.
    pub speaker: Speaker,
    /// Turn text.
    pub text: String,
}

/// One current claim surfaced as trusted memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryClaim {
    /// Claim id.
    pub claim_id: String,
    /// Claim text.
    pub text: String,
    /// Source document path.
    pub source_path: String,
    /// One-based line.
    pub line: u32,
    /// Byte offset start.
    pub char_start: u32,
    /// Byte offset end.
    pub char_end: u32,
}

/// A retired memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleMemory {
    /// Superseded claim id.
    pub claim_id: String,
    /// Outdated text.
    pub text: String,
    /// Replacement claim id.
    pub superseded_by: String,
    /// Replacement text.
    pub superseded_by_text: String,
}

/// An unresolved memory conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConflict {
    /// First claim text.
    pub claim_a_text: String,
    /// Second claim text.
    pub claim_b_text: String,
    /// Conflict reason.
    pub reason: String,
}

/// Full memory snapshot consumed by the chat prompt builder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySnapshot {
    /// Workspace id.
    pub workspace_id: String,
    /// Replay frontier.
    pub replayed_through_sequence: u64,
    /// Current trusted memory.
    pub current: Vec<MemoryClaim>,
    /// Superseded memory.
    pub stale: Vec<StaleMemory>,
    /// Open conflicts.
    pub conflicts: Vec<MemoryConflict>,
}

/// Section header for trusted memory in the system prompt.
pub const CURRENT_MEMORY_HEADER: &str = "## Current memory (trusted)";
/// Section header for superseded memory in the system prompt.
pub const OUTDATED_MEMORY_HEADER: &str = "## Outdated memory (do NOT trust — superseded)";
/// Section header for conflicting memory in the system prompt.
pub const CONFLICT_MEMORY_HEADER: &str = "## Conflicting memory (unresolved)";

/// Build the system prompt from a replayed memory snapshot.
///
/// Current claims are injected as trusted memory with `path:line` provenance;
/// superseded claims are listed as outdated with what replaced them; open
/// conflicts are surfaced so the model flags them instead of picking a side.
/// Deterministic over the snapshot.
#[must_use]
pub fn build_system_prompt(memory: &MemorySnapshot) -> String {
    let mut prompt = String::from(
        "You are a personal assistant with persistent, versioned memory. Your memory is a \
         texo claim-chain: facts from previous sessions were extracted into claims, and when \
         a fact changed the old claim was retired (superseded) with a receipt instead of \
         accumulating. Everything below is replayed from that journal.\n",
    );

    prompt.push('\n');
    prompt.push_str(CURRENT_MEMORY_HEADER);
    prompt.push('\n');
    if memory.current.is_empty() {
        prompt.push_str("(empty — no remembered facts yet; say so when memory is asked for.)\n");
    } else {
        for claim in &memory.current {
            writeln!(
                &mut prompt,
                "- {} [{}:{}]",
                claim.text, claim.source_path, claim.line
            )
            .expect("writing to a String cannot fail");
        }
    }

    prompt.push('\n');
    prompt.push_str(OUTDATED_MEMORY_HEADER);
    prompt.push('\n');
    if memory.stale.is_empty() {
        prompt.push_str("(none)\n");
    } else {
        for stale in &memory.stale {
            writeln!(
                &mut prompt,
                "- \"{}\" — superseded by: \"{}\"",
                stale.text, stale.superseded_by_text
            )
            .expect("writing to a String cannot fail");
        }
    }

    if !memory.conflicts.is_empty() {
        prompt.push('\n');
        prompt.push_str(CONFLICT_MEMORY_HEADER);
        prompt.push('\n');
        for conflict in &memory.conflicts {
            writeln!(
                &mut prompt,
                "- \"{}\" vs \"{}\"",
                conflict.claim_a_text, conflict.claim_b_text
            )
            .expect("writing to a String cannot fail");
        }
    }

    prompt.push_str(
        "\nRules:\n\
         - Answer from current memory when relevant, citing the source like (file.md:12).\n\
         - If memory is empty or does not cover the question, say you do not have that in \
         memory instead of inventing a remembered fact.\n\
         - Never present outdated memory as true. If asked, explain it was superseded and by \
         what.\n\
         - If memories conflict, say so and present both sides.\n\
         - Keep replies concise.\n",
    );
    prompt
}

/// Build the OpenAI-compatible chat-completions request body: system prompt
/// first, then the session's turns in order, then the new user message.
#[must_use]
pub fn build_chat_request(
    model: &str,
    system_prompt: &str,
    history: &[Utterance],
    user_message: &str,
) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": system_prompt })];
    for utterance in history {
        messages.push(json!({
            "role": utterance.speaker.role(),
            "content": utterance.text,
        }));
    }
    messages.push(json!({ "role": "user", "content": user_message }));
    json!({
        "model": model,
        "max_tokens": MAX_REPLY_TOKENS,
        "messages": messages,
    })
}

/// Extract the assistant reply from a chat-completions response, defensively.
///
/// # Errors
///
/// Returns [`TexoError::Model`] when `choices[0].message.content` is absent or
/// not a string.
pub fn parse_chat_reply(value: &Value) -> Result<String, TexoError> {
    value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| model_error("chat response had no choices[0].message.content string"))
}

/// POST the request to `{base_url}/chat/completions` and return the assistant
/// reply.
///
/// # Errors
///
/// Returns [`TexoError::Model`] for client construction, HTTP, JSON, or response
/// shape failures.
pub fn complete(config: &ChatConfig, body: &Value) -> Result<String, TexoError> {
    let client = OpenAiCompatClient::from_env_vars(
        Some(config.api_key.clone()),
        Some(config.base_url.clone()),
    )
    .map_err(|error| model_from_texo(&error))?;
    let value = client
        .post_json("/chat/completions", body)
        .map_err(|error| model_from_texo(&error))?;
    parse_chat_reply(&value)
}

fn model_from_texo(error: &TexoError) -> TexoError {
    model_error(error.to_string())
}

fn model_error(detail: impl Into<String>) -> TexoError {
    TexoError::Model {
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(
        current: Vec<MemoryClaim>,
        stale: Vec<StaleMemory>,
        conflicts: Vec<MemoryConflict>,
    ) -> MemorySnapshot {
        MemorySnapshot {
            workspace_id: "memory".to_owned(),
            replayed_through_sequence: 7,
            current,
            stale,
            conflicts,
        }
    }

    fn claim(text: &str, path: &str, line: u32) -> MemoryClaim {
        MemoryClaim {
            claim_id: "claim_aaaaaaaaaaaa".to_owned(),
            text: text.to_owned(),
            source_path: path.to_owned(),
            line,
            char_start: 0,
            char_end: 0,
        }
    }

    #[test]
    fn config_from_env_vars_matches_old_precedence() {
        assert!(ChatConfig::from_env_vars(None, None, None).is_none());
        let config = ChatConfig::from_env_vars(
            Some("k".to_string()),
            Some("https://example.com/api/".to_string()),
            Some("chat/model".to_string()),
        )
        .expect("api key present");
        assert_eq!(config.base_url, "https://example.com/api");
        assert_eq!(config.model, "chat/model");

        let defaulted =
            ChatConfig::from_env_vars(Some("k".to_string()), None, None).expect("api key present");
        assert_eq!(defaulted.base_url, DEFAULT_BASE_URL);
        assert_eq!(defaulted.model, DEFAULT_CHAT_MODEL);
    }

    #[test]
    fn system_prompt_injects_current_claims_with_provenance() {
        let memory = snapshot(
            vec![claim("Deploys moved to Tuesday.", "session-2.md", 3)],
            vec![],
            vec![],
        );
        let prompt = build_system_prompt(&memory);
        assert!(prompt.contains(CURRENT_MEMORY_HEADER));
        assert!(prompt.contains("- Deploys moved to Tuesday. [session-2.md:3]"));
        assert!(prompt.contains("(none)"), "empty stale section is explicit");
    }

    #[test]
    fn system_prompt_flags_superseded_claims_as_outdated_only() {
        let memory = snapshot(
            vec![claim("Deploys moved to Tuesday.", "session-2.md", 3)],
            vec![StaleMemory {
                claim_id: "claim_bbbbbbbbbbbb".to_owned(),
                text: "Deploys happen on Friday.".to_owned(),
                superseded_by: "claim_aaaaaaaaaaaa".to_owned(),
                superseded_by_text: "Deploys moved to Tuesday.".to_owned(),
            }],
            vec![],
        );
        let prompt = build_system_prompt(&memory);
        let outdated_at = prompt
            .find(OUTDATED_MEMORY_HEADER)
            .expect("outdated section present");
        let trusted = &prompt[..outdated_at];
        assert!(trusted.contains("Deploys moved to Tuesday."));
        assert!(
            !trusted.contains("Deploys happen on Friday."),
            "superseded text must not appear in the trusted section"
        );
        let outdated = &prompt[outdated_at..];
        assert!(outdated.contains(
            "\"Deploys happen on Friday.\" — superseded by: \"Deploys moved to Tuesday.\""
        ));
    }

    #[test]
    fn system_prompt_says_memory_is_empty() {
        let prompt = build_system_prompt(&snapshot(vec![], vec![], vec![]));
        assert!(prompt.contains("(empty — no remembered facts yet"));
    }

    #[test]
    fn system_prompt_lists_conflicts_when_present() {
        let memory = snapshot(
            vec![],
            vec![],
            vec![MemoryConflict {
                claim_a_text: "Releases happen on Monday.".to_owned(),
                claim_b_text: "Releases go out on Friday.".to_owned(),
                reason: "contradictory values".to_owned(),
            }],
        );
        let prompt = build_system_prompt(&memory);
        assert!(prompt.contains(CONFLICT_MEMORY_HEADER));
        assert!(prompt.contains("\"Releases happen on Monday.\" vs \"Releases go out on Friday.\""));
    }

    #[test]
    fn chat_request_orders_system_history_then_user() {
        let history = vec![
            Utterance {
                speaker: Speaker::User,
                text: "hello".to_owned(),
            },
            Utterance {
                speaker: Speaker::Assistant,
                text: "hi!".to_owned(),
            },
        ];
        let body = build_chat_request("some/model", "SYSTEM", &history, "what do you remember?");
        assert_eq!(body["model"], "some/model");
        assert_eq!(body["max_tokens"], 1024);
        let messages = body["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "SYSTEM");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "hello");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[2]["content"], "hi!");
        assert_eq!(messages[3]["role"], "user");
        assert_eq!(messages[3]["content"], "what do you remember?");
    }

    #[test]
    fn chat_reply_parses_content_and_rejects_missing() {
        let ok = serde_json::json!({
            "choices": [ { "message": { "role": "assistant", "content": "Tuesday." } } ]
        });
        assert_eq!(parse_chat_reply(&ok).expect("reply"), "Tuesday.");

        let missing = serde_json::json!({ "choices": [] });
        let err = parse_chat_reply(&missing).expect_err("no content");
        assert!(err.to_string().contains("choices[0].message.content"));
    }
}
