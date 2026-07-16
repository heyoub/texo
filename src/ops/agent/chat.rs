//! Agent memory projection and model-bound chat composition.

use crate::claims::session_log::TurnEntry;
use crate::claims::workspace::WorkspaceView;
use crate::error::TexoError;
use crate::ops::env;

use super::{MemoryClaim, MemoryConflict, MemorySnapshot, StaleMemory};

pub(super) fn memory_snapshot(view: &WorkspaceView) -> MemorySnapshot {
    let current = view
        .claims
        .iter()
        .filter(|claim| claim.card.phase != 2)
        .map(|claim| MemoryClaim {
            claim_id: claim.card.claim_id.clone(),
            text: claim.card.text.clone(),
            source_path: claim.card.source_path.clone(),
            line: claim.card.line_start,
            char_start: claim.card.char_start,
            char_end: claim.card.char_end,
        })
        .collect::<Vec<_>>();
    let stale = view
        .claims
        .iter()
        .filter(|claim| claim.card.phase == 2)
        .filter_map(|claim| {
            claim.card.superseded_by.as_ref().map(|superseded_by| {
                let superseded_by_text = claim_text(view, superseded_by);
                StaleMemory {
                    claim_id: claim.card.claim_id.clone(),
                    text: claim.card.text.clone(),
                    superseded_by: superseded_by.clone(),
                    superseded_by_text,
                }
            })
        })
        .collect::<Vec<_>>();
    let conflicts = view
        .conflicts
        .iter()
        .filter(|conflict| conflict.phase == 1)
        .map(|conflict| MemoryConflict {
            claim_a_text: claim_text(view, &conflict.claim_a),
            claim_b_text: claim_text(view, &conflict.claim_b),
            reason: conflict.reason.clone(),
        })
        .collect::<Vec<_>>();
    MemorySnapshot {
        workspace_id: view.workspace_id.clone(),
        replayed_through_sequence: view.frontier,
        current,
        stale,
        conflicts,
    }
}

fn claim_text(view: &WorkspaceView, claim_id: &str) -> String {
    view.claims
        .iter()
        .find(|claim| claim.card.claim_id == claim_id)
        .map_or_else(String::new, |claim| claim.card.text.clone())
}

pub(super) fn model_role_enabled(role: crate::gateway::ModelRole) -> Result<bool, TexoError> {
    let resolved = env::with(|op_env| {
        crate::gateway::resolve_role(
            role,
            &crate::gateway::RoleOverrides::default(),
            op_env.config.gateway.as_ref(),
        )
    })?;
    Ok(crate::host::grants_model_capability(Some(
        resolved.api_key.as_str(),
    )))
}

#[cfg(feature = "openrouter")]
pub(super) fn complete_chat(
    memory: &MemorySnapshot,
    history: &[TurnEntry],
    user_message: &str,
) -> Result<String, TexoError> {
    let role = env::with(|op_env| {
        crate::gateway::resolve_role(
            crate::gateway::ModelRole::Chat,
            &crate::gateway::RoleOverrides::default(),
            op_env.config.gateway.as_ref(),
        )
    })?;
    if !role.is_enabled() {
        return Err(TexoError::Model {
            detail: "chat is disabled: TEXO_LLM_API_KEY is not set".to_string(),
        });
    }
    let chat_memory = to_chat_memory(memory);
    let chat_history = history
        .iter()
        .filter_map(|turn| {
            let speaker = match turn.speaker.as_str() {
                "user" => crate::semantics::chat::Speaker::User,
                "assistant" => crate::semantics::chat::Speaker::Assistant,
                _ => return None,
            };
            Some(crate::semantics::chat::Utterance {
                speaker,
                text: turn.text.clone(),
            })
        })
        .collect::<Vec<_>>();
    let system_prompt = crate::semantics::chat::build_system_prompt(&chat_memory);
    let body = crate::semantics::chat::build_chat_request(
        &role,
        &system_prompt,
        &chat_history,
        user_message,
    );
    crate::semantics::chat::complete(&role, &body)
}

#[cfg(not(feature = "openrouter"))]
pub(super) fn complete_chat(
    _memory: &MemorySnapshot,
    _history: &[TurnEntry],
    _user_message: &str,
) -> Result<String, TexoError> {
    Err(TexoError::Model {
        detail: "chat is disabled: openrouter feature is disabled".to_string(),
    })
}

#[cfg(feature = "openrouter")]
fn to_chat_memory(memory: &MemorySnapshot) -> crate::semantics::chat::MemorySnapshot {
    crate::semantics::chat::MemorySnapshot {
        workspace_id: memory.workspace_id.clone(),
        replayed_through_sequence: memory.replayed_through_sequence,
        current: memory
            .current
            .iter()
            .map(|claim| crate::semantics::chat::MemoryClaim {
                claim_id: claim.claim_id.clone(),
                text: claim.text.clone(),
                source_path: claim.source_path.clone(),
                line: claim.line,
                char_start: claim.char_start,
                char_end: claim.char_end,
            })
            .collect(),
        stale: memory
            .stale
            .iter()
            .map(|stale| crate::semantics::chat::StaleMemory {
                claim_id: stale.claim_id.clone(),
                text: stale.text.clone(),
                superseded_by: stale.superseded_by.clone(),
                superseded_by_text: stale.superseded_by_text.clone(),
            })
            .collect(),
        conflicts: memory
            .conflicts
            .iter()
            .map(|conflict| crate::semantics::chat::MemoryConflict {
                claim_a_text: conflict.claim_a_text.clone(),
                claim_b_text: conflict.claim_b_text.clone(),
                reason: conflict.reason.clone(),
            })
            .collect(),
    }
}
