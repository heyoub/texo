//! Chat-completions plumbing.
//!
//! The memory-grounded system prompt and the request body are built by pure
//! functions (unit-tested, no network). Only [`complete`] talks HTTP, against
//! any OpenAI-compatible `/chat/completions` endpoint using the same
//! environment conventions as texo-semantics (`OPENROUTER_BASE_URL`,
//! `OPENROUTER_API_KEY`) plus `OPENROUTER_CHAT_MODEL` for the chat role.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::memory::MemorySnapshot;
use crate::session::Utterance;

/// Default chat model (an OpenRouter slug, like the other role defaults).
/// Override with `OPENROUTER_CHAT_MODEL` — e.g. `qwen3.7-max` when
/// `OPENROUTER_BASE_URL` points at Qwen Cloud's DashScope compatible mode.
pub const DEFAULT_CHAT_MODEL: &str = "anthropic/claude-opus-4.8";
/// Default OpenAI-compatible base URL (same default as texo-semantics).
const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
/// Environment variable holding the bearer token.
const ENV_API_KEY: &str = "OPENROUTER_API_KEY";
/// Environment variable overriding the API base URL.
const ENV_BASE_URL: &str = "OPENROUTER_BASE_URL";
/// Environment variable overriding the chat model.
const ENV_CHAT_MODEL: &str = "OPENROUTER_CHAT_MODEL";
/// Completion-token ceiling for one assistant reply.
const MAX_REPLY_TOKENS: u32 = 1024;
/// Bytes of an error body kept for diagnostics.
const MAX_ERROR_BODY: usize = 2048;

/// Resolved chat backend configuration.
#[derive(Debug, Clone)]
pub struct ChatConfig {
    /// OpenAI-compatible API base URL.
    pub base_url: String,
    /// Bearer token.
    pub api_key: String,
    /// Chat model id.
    pub model: String,
}

impl ChatConfig {
    /// Resolve from the environment. `None` when no API key is set — chat is
    /// disabled but the memory endpoints keep working.
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var(ENV_API_KEY)
            .ok()
            .filter(|key| !key.trim().is_empty())?;
        let base_url = std::env::var(ENV_BASE_URL)
            .ok()
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        let model = std::env::var(ENV_CHAT_MODEL)
            .ok()
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CHAT_MODEL.to_owned());
        Some(Self {
            base_url,
            api_key,
            model,
        })
    }
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
            prompt.push_str(&format!(
                "- {} [{}:{}]\n",
                claim.text, claim.source_path, claim.line
            ));
        }
    }

    prompt.push('\n');
    prompt.push_str(OUTDATED_MEMORY_HEADER);
    prompt.push('\n');
    if memory.stale.is_empty() {
        prompt.push_str("(none)\n");
    } else {
        for stale in &memory.stale {
            prompt.push_str(&format!(
                "- \"{}\" — superseded by: \"{}\"\n",
                stale.text, stale.superseded_by_text
            ));
        }
    }

    if !memory.conflicts.is_empty() {
        prompt.push('\n');
        prompt.push_str(CONFLICT_MEMORY_HEADER);
        prompt.push('\n');
        for conflict in &memory.conflicts {
            prompt.push_str(&format!(
                "- \"{}\" vs \"{}\"\n",
                conflict.claim_a_text, conflict.claim_b_text
            ));
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

/// Extract the assistant reply from a chat-completions response, defensively:
/// a missing `choices[0].message.content` string is a structured error, never
/// a panic.
pub fn parse_chat_reply(value: &Value) -> Result<String> {
    value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("chat response had no choices[0].message.content string"))
}

/// POST the request to `{base_url}/chat/completions` and return the assistant
/// reply. Untested by design (env-gated model behavior); the request body it
/// sends comes from the unit-tested [`build_chat_request`].
pub async fn complete(
    client: &reqwest::Client,
    config: &ChatConfig,
    body: &Value,
) -> Result<String> {
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let response = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .json(body)
        .send()
        .await
        .with_context(|| format!("requesting {url}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .with_context(|| format!("reading chat response from {url}"))?;
    if !status.is_success() {
        let mut detail = text;
        detail.truncate(MAX_ERROR_BODY);
        bail!("chat backend returned {status}: {detail}");
    }
    let value: Value = serde_json::from_str(&text).context("chat response was not valid JSON")?;
    parse_chat_reply(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryClaim, MemoryConflict, StaleMemory};
    use crate::session::Speaker;

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
