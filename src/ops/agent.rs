//! Agent/session operations.
//!
//! DESIGN: batpak lanes are invisible to default-lane queries by construction
//! (proven in spike 5), so per-turn writes need NO visibility fence: a
//! `SessionTurnV1` on `session_lane(session_id)` is durable at append
//! (crash-safe mid-session) and hidden from every lane-0 projection. Session
//! end does not "promote" turns -- turns stay in their lane as the transcript
//! archive; what lands on lane 0 is the EXTRACTED CLAIMS via ingest. There is
//! no in-memory session state anywhere: the journal is the session.
//!
//! A failed session-end leaves the lane archive intact and may be retried; no
//! transcript restore path is needed.
#![expect(
    missing_docs,
    reason = "syncbat::operation generates public registration shims without doc injection hooks"
)]

use batpak::event::EventPayload;
use batpak::id::{EntityIdType, IdempotencyKey};
use batpak::store::{AppendOptions, AppendPositionHint};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use syncbat::{CoreBuilder, HandlerResult, OperationRegisterItem};

use crate::claims::session_log::TurnEntry;
use crate::claims::workspace::WorkspaceView;
use crate::error::TexoError;
use crate::events::coordinate::{coordinate_for_session, entity_for_session, session_lane};
use crate::events::payloads::{
    ClaimRecordedV2, ClaimSupersededV2, SessionTurnV1, SourceObservedV2,
};
use crate::journal_store::JournalStore;
use crate::ops::env::{self, ReceiptNote};
use crate::ops::handlers::{
    append_json, assemble_current_view, infer_supersessions, op_runtime, parse_input, plan_sources,
    run_op, run_relate_pass, take_receipts, workspace_temporal_policy,
};

/// Directory under the workspace root where session transcripts land.
pub const SESSIONS_DIR: &str = "sessions";
/// Maximum accepted session id length.
pub const MAX_SESSION_ID_LEN: usize = 64;

/// Who spoke one session turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    /// User-authored turn.
    User,
    /// Assistant-authored turn.
    Assistant,
}

impl Speaker {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::User => "User: ",
            Self::Assistant => "Assistant: ",
        }
    }
}

/// Whether a session id is safe as a path stem and lane key.
#[must_use]
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SESSION_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Stable transcript path for one session.
#[must_use]
pub fn session_doc_path(root: &Path, session_id: &str) -> PathBuf {
    root.join(SESSIONS_DIR).join(format!("{session_id}.md"))
}

/// Journal one turn directly to a session lane.
///
/// # Errors
///
/// Returns [`TexoError::OpInput`] for an invalid session id;
/// [`TexoError::Store`] when append or receipt verification fails.
pub fn journal_turn(
    store: &JournalStore,
    workspace_id: &str,
    session_id: &str,
    speaker: Speaker,
    text: &str,
    observed_at_ms: u64,
) -> Result<ReceiptNote, TexoError> {
    let payload = next_turn_payload(
        store,
        workspace_id,
        session_id,
        speaker,
        text,
        observed_at_ms,
    )?;
    append_turn_direct(store, &payload)
}

/// Read all turns in a session lane, sorted by turn number.
///
/// # Errors
///
/// Returns [`TexoError::Store`] when event reads fail and
/// [`TexoError::Decode`] when a lane event cannot be decoded as a turn.
pub fn read_session_turns(
    store: &JournalStore,
    session_id: &str,
) -> Result<Vec<TurnEntry>, TexoError> {
    let entity = entity_for_session(session_id);
    let lane = session_lane(session_id);
    let mut turns = Vec::new();
    for entry in store.stream_lane(&entity, lane) {
        if entry.event_kind() != <SessionTurnV1 as EventPayload>::KIND {
            continue;
        }
        let raw = store.read_raw(entry.event_id())?;
        let payload: SessionTurnV1 =
            batpak::encoding::from_bytes(&raw.event.payload).map_err(|error| {
                TexoError::Decode {
                    entity: entity.clone(),
                    detail: error.to_string(),
                }
            })?;
        turns.push(TurnEntry {
            session_id: payload.session_id,
            workspace_id: payload.workspace_id,
            speaker: payload.speaker,
            text: payload.text,
            turn_no: payload.turn_no,
            observed_at_ms: payload.observed_at_ms,
        });
    }
    turns.sort_by_key(|turn| turn.turn_no);
    Ok(turns)
}

/// Render a session transcript as markdown.
#[must_use]
pub fn render_transcript(session_id: &str, turns: &[TurnEntry], include_assistant: bool) -> String {
    let mut out = format!("# Session {session_id}\n");
    for turn in turns {
        let speaker = match turn.speaker.as_str() {
            "user" => Speaker::User,
            "assistant" => {
                if !include_assistant {
                    continue;
                }
                Speaker::Assistant
            }
            _ => continue,
        };
        let clean = turn.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if clean.is_empty() {
            continue;
        }
        out.push('\n');
        out.push_str(speaker.prefix());
        out.push_str(&clean);
        out.push('\n');
    }
    out
}

#[syncbat::operation(
    descriptor = AGENT_CHAT,
    register = register_agent_chat,
    register_item = agent_chat_item,
    name = "texo.agent.chat",
    effect = Persist,
    input_schema = "texo.agent.chat.input.v2",
    output_schema = "texo.agent.chat.output.v2",
    receipt_kind = "receipt.texo.agent.chat.v2",
    appends_events = ["evt.e008"],
    reads_events = ["evt.e008"],
    queries_projections = ["texo.workspace.view.v2"],
    requires_capabilities = ["texo.cap.model"]
)]
#[tracing::instrument(skip_all)]
fn agent_chat(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.agent.chat", || {
        let input: AgentChatInput = parse_input("texo.agent.chat", input)?;
        validate_session_input("texo.agent.chat", &input.session_id)?;
        if input.message.trim().is_empty() {
            return Err(TexoError::OpInput {
                op: "texo.agent.chat".to_string(),
                detail: "empty message".to_string(),
            });
        }
        cx.event_read_handle()
            .read_event("evt.e008")
            .map_err(|error| op_runtime("texo.agent.chat", error))?;
        cx.projection_read_handle()
            .query_projection("texo.workspace.view.v2")
            .map_err(|error| op_runtime("texo.agent.chat", error))?;

        let user = env::with(|op_env| {
            next_turn_payload(
                &op_env.store,
                &op_env.workspace_id,
                &input.session_id,
                Speaker::User,
                &input.message,
                input.observed_at_ms,
            )
        })??;
        append_json(
            "texo.agent.chat",
            cx,
            <SessionTurnV1 as EventPayload>::KIND,
            &user,
        )?;

        let (memory, history) = env::with(|op_env| {
            let mut cache = op_env.cache.borrow_mut();
            let view = crate::claims::workspace::assemble(
                &op_env.store,
                &op_env.workspace_id,
                &mut cache,
            )?;
            let memory = memory_snapshot(&view);
            let history = read_session_turns(&op_env.store, &input.session_id)?;
            Ok::<_, TexoError>((memory, history))
        })??;
        let reply = complete_chat(&memory, &history, &input.message)?;
        let assistant = env::with(|op_env| {
            next_turn_payload(
                &op_env.store,
                &op_env.workspace_id,
                &input.session_id,
                Speaker::Assistant,
                &reply,
                input.observed_at_ms,
            )
        })??;
        append_json(
            "texo.agent.chat",
            cx,
            <SessionTurnV1 as EventPayload>::KIND,
            &assistant,
        )?;
        drop(take_receipts()?);
        Ok(AgentChatOutput {
            reply,
            memory_used: memory.current,
        })
    })
}

#[syncbat::operation(
    descriptor = AGENT_MEMORY,
    register = register_agent_memory,
    register_item = agent_memory_item,
    name = "texo.agent.memory",
    effect = Inspect,
    input_schema = "texo.agent.memory.input.v2",
    output_schema = "texo.agent.memory.output.v2",
    receipt_kind = "receipt.texo.agent.memory.v2",
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn agent_memory(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.agent.memory", || {
        let _input: AgentMemoryInput = parse_input("texo.agent.memory", input)?;
        cx.projection_read_handle()
            .query_projection("texo.workspace.view.v2")
            .map_err(|error| op_runtime("texo.agent.memory", error))?;
        let view = assemble_current_view()?;
        Ok(memory_snapshot(&view))
    })
}

#[syncbat::operation(
    descriptor = AGENT_SESSION_END,
    register = register_agent_session_end,
    register_item = agent_session_end_item,
    name = "texo.agent.session.end",
    effect = Persist,
    input_schema = "texo.agent.session.end.input.v2",
    output_schema = "texo.agent.session.end.output.v3",
    receipt_kind = "receipt.texo.agent.session.end.v2",
    appends_events = ["evt.e001", "evt.e002", "evt.e003", "evt.e004", "evt.e009", "evt.e00a"],
    reads_events = ["evt.e008"],
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
#[expect(
    clippy::too_many_lines,
    reason = "session settlement keeps transcript, ingest, and relate ordering in one operation"
)]
fn agent_session_end(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.agent.session.end", || {
        let input: AgentSessionEndInput = parse_input("texo.agent.session.end", input)?;
        validate_session_input("texo.agent.session.end", &input.session_id)?;
        cx.event_read_handle()
            .read_event("evt.e008")
            .map_err(|error| op_runtime("texo.agent.session.end", error))?;
        cx.projection_read_handle()
            .query_projection("texo.workspace.view.v2")
            .map_err(|error| op_runtime("texo.agent.session.end", error))?;
        let turns = env::with(|op_env| read_session_turns(&op_env.store, &input.session_id))??;
        if turns.is_empty() {
            return Err(TexoError::MissingEntity {
                entity: entity_for_session(&input.session_id),
            });
        }
        let (root, workspace_id, extractor_cmd) = env::with(|op_env| {
            (
                op_env.root.clone(),
                op_env.workspace_id.clone(),
                op_env.config.extractor_cmd.clone(),
            )
        })?;
        let doc_path = session_doc_path(&root, &input.session_id);
        if let Some(parent) = doc_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            &doc_path,
            render_transcript(&input.session_id, &turns, false),
        )?;
        let view = assemble_current_view()?;
        let planned = plan_sources(
            "texo.agent.session.end",
            &root,
            &doc_path,
            &workspace_id,
            input.observed_at_ms,
            extractor_cmd.as_deref(),
            &view,
        )?;
        if !planned.skipped.is_empty() {
            return Err(TexoError::Source {
                path: doc_path.to_string_lossy().to_string(),
                detail: "generated session transcript could not be planned".to_string(),
            });
        }
        let new_claims = planned
            .sources
            .iter()
            .flat_map(|source| source.claims.iter().cloned())
            .collect::<Vec<_>>();
        let temporal = workspace_temporal_policy(&view)?;
        let supersessions =
            infer_supersessions(&view, &new_claims, input.observed_at_ms, &temporal)?;
        for source in &planned.sources {
            append_json(
                "texo.agent.session.end",
                cx,
                <SourceObservedV2 as EventPayload>::KIND,
                &source.observed,
            )?;
            for claim in &source.claims {
                append_json(
                    "texo.agent.session.end",
                    cx,
                    <ClaimRecordedV2 as EventPayload>::KIND,
                    claim,
                )?;
            }
        }
        for supersession in &supersessions.applied {
            append_json(
                "texo.agent.session.end",
                cx,
                <ClaimSupersededV2 as EventPayload>::KIND,
                supersession,
            )?;
        }
        drop(take_receipts()?);
        let relate = if model_role_enabled(crate::gateway::ModelRole::Relate)? {
            let out = run_relate_pass("texo.agent.session.end", cx, input.observed_at_ms, false)?;
            RelateOutcome::Ran {
                supersessions: out.supersessions.len(),
                conflicts: out.conflicts.len(),
            }
        } else {
            RelateOutcome::Skipped {
                reason: "TEXO_LLM_API_KEY is not set".to_string(),
            }
        };
        Ok(SessionEndReport {
            session_id: input.session_id,
            doc_path: doc_path
                .strip_prefix(&root)
                .unwrap_or(&doc_path)
                .to_string_lossy()
                .to_string(),
            sources_observed: u32::try_from(planned.sources.len()).unwrap_or(u32::MAX),
            claims_recorded: u32::try_from(new_claims.len()).unwrap_or(u32::MAX),
            ingest_supersessions: u32::try_from(supersessions.applied.len()).unwrap_or(u32::MAX),
            supersessions_held: supersessions.held.len(),
            held_supersessions: supersessions.held,
            relate,
        })
    })
}

#[syncbat::operation(
    descriptor = SESSION_EXPORT,
    register = register_session_export,
    register_item = session_export_item,
    name = "texo.session.export",
    effect = Inspect,
    input_schema = "texo.session.export.input.v2",
    output_schema = "texo.session.export.output.v2",
    receipt_kind = "receipt.texo.session.export.v2",
    reads_events = ["evt.e008"]
)]
#[tracing::instrument(skip_all)]
fn session_export(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.session.export", || {
        let input: SessionExportInput = parse_input("texo.session.export", input)?;
        validate_session_input("texo.session.export", &input.session_id)?;
        cx.event_read_handle()
            .read_event("evt.e008")
            .map_err(|error| op_runtime("texo.session.export", error))?;
        let turns = env::with(|op_env| read_session_turns(&op_env.store, &input.session_id))??;
        if turns.is_empty() {
            return Err(TexoError::MissingEntity {
                entity: entity_for_session(&input.session_id),
            });
        }
        Ok(SessionExportOutput {
            session_id: input.session_id.clone(),
            markdown: render_transcript(&input.session_id, &turns, true),
        })
    })
}

/// Return the agent operation registration items.
#[must_use]
pub fn catalog() -> Vec<OperationRegisterItem> {
    vec![
        agent_chat_item(),
        agent_memory_item(),
        agent_session_end_item(),
        session_export_item(),
    ]
}

/// Register all agent operations.
///
/// # Errors
///
/// Returns [`syncbat::BuildError`] if a descriptor or handler cannot be
/// registered with the builder.
pub fn register_all(builder: &mut CoreBuilder) -> Result<(), syncbat::BuildError> {
    register_agent_chat(builder)?;
    register_agent_memory(builder)?;
    register_agent_session_end(builder)?;
    register_session_export(builder)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct AgentChatInput {
    session_id: String,
    message: String,
    observed_at_ms: u64,
}

#[derive(Debug, Serialize)]
struct AgentChatOutput {
    reply: String,
    memory_used: Vec<MemoryClaim>,
}

#[derive(Debug, Deserialize)]
struct AgentMemoryInput {}

#[derive(Debug, Deserialize)]
struct AgentSessionEndInput {
    session_id: String,
    observed_at_ms: u64,
}

#[derive(Debug, Deserialize)]
struct SessionExportInput {
    session_id: String,
}

#[derive(Debug, Serialize)]
struct SessionExportOutput {
    session_id: String,
    markdown: String,
}

/// One current claim surfaced as trusted memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryConflict {
    /// First claim text.
    pub claim_a_text: String,
    /// Second claim text.
    pub claim_b_text: String,
    /// Conflict reason.
    pub reason: String,
}

/// Full memory snapshot replayed from the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

/// Result of ending one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionEndReport {
    /// Session id.
    pub session_id: String,
    /// Transcript path relative to the workspace root.
    pub doc_path: String,
    /// New sources journaled by ingest.
    pub sources_observed: u32,
    /// New claims journaled by ingest.
    pub claims_recorded: u32,
    /// Supersessions journaled during ingest.
    pub ingest_supersessions: u32,
    /// Explicit replacements held because source order was not authoritative.
    pub supersessions_held: usize,
    /// Typed held-pair evidence for retry after source indexing.
    pub held_supersessions: Vec<crate::ops::handlers::HeldExplicitSupersession>,
    /// Relate outcome.
    pub relate: RelateOutcome,
}

/// What happened to the relate pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RelateOutcome {
    /// The pass ran and journaled any derived facts.
    Ran {
        /// Supersession count.
        supersessions: usize,
        /// Conflict count.
        conflicts: usize,
    },
    /// The pass was skipped.
    Skipped {
        /// Skip reason.
        reason: String,
    },
}

fn validate_session_input(op: &str, session_id: &str) -> Result<(), TexoError> {
    if valid_session_id(session_id) {
        Ok(())
    } else {
        Err(TexoError::OpInput {
            op: op.to_string(),
            detail: "invalid session_id: use 1-64 ASCII letters, digits, '-' or '_'".to_string(),
        })
    }
}

fn next_turn_payload(
    store: &JournalStore,
    workspace_id: &str,
    session_id: &str,
    speaker: Speaker,
    text: &str,
    observed_at_ms: u64,
) -> Result<SessionTurnV1, TexoError> {
    validate_session_input("texo.agent.turn", session_id)?;
    let existing = store
        .stream_lane(&entity_for_session(session_id), session_lane(session_id))
        .len();
    let next = existing
        .checked_add(1)
        .ok_or_else(|| TexoError::OpRuntime {
            op: "texo.agent.turn".to_string(),
            detail: "session turn count overflow".to_string(),
            denied: false,
        })?;
    let turn_no = u32::try_from(next).map_err(|_| TexoError::OpRuntime {
        op: "texo.agent.turn".to_string(),
        detail: "session turn count exceeded u32".to_string(),
        denied: false,
    })?;
    Ok(SessionTurnV1 {
        session_id: session_id.to_string(),
        workspace_id: workspace_id.to_string(),
        speaker: speaker.as_str().to_string(),
        text: text.to_string(),
        turn_no,
        observed_at_ms,
    })
}

fn append_turn_direct(
    store: &JournalStore,
    payload: &SessionTurnV1,
) -> Result<ReceiptNote, TexoError> {
    let coordinate = coordinate_for_session(&payload.workspace_id, &payload.session_id)?;
    let entity = entity_for_session(&payload.session_id);
    let lane = session_lane(&payload.session_id);
    let depth = u32::try_from(store.stream_lane(&entity, lane).len()).map_err(|_| {
        TexoError::OpRuntime {
            op: "texo.agent.turn".to_string(),
            detail: "session lane depth exceeded u32".to_string(),
            denied: false,
        }
    })?;
    let hint = if depth == 0 {
        AppendPositionHint::branch_root(lane, 0)
    } else {
        AppendPositionHint::new(lane, depth)
    };
    let receipt = store.append_typed_with_options(
        &coordinate,
        payload,
        AppendOptions::new()
            .with_position_hint(hint)
            .with_idempotency(IdempotencyKey::for_operation(
                "texo.session.turn.v1",
                &[
                    &payload.workspace_id,
                    &payload.session_id,
                    &payload.turn_no.to_string(),
                ],
            )),
    )?;
    let verification = store.verify_append_receipt(&receipt);
    if !verification.is_valid() {
        return Err(TexoError::ReceiptInvalid {
            event_id: format!("{:032x}", receipt.event_id.as_u128()),
            reason: verification.error().map_or_else(
                || "invalid receipt".to_string(),
                |error| format!("{error:?}"),
            ),
        });
    }
    Ok(ReceiptNote {
        event_id_hex: format!("{:032x}", receipt.event_id.as_u128()),
        kind_bits: <SessionTurnV1 as EventPayload>::KIND.as_raw_u16(),
        global_sequence: receipt.global_sequence,
    })
}

fn memory_snapshot(view: &WorkspaceView) -> MemorySnapshot {
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
                let superseded_by_text = view
                    .claims
                    .iter()
                    .find(|candidate| candidate.card.claim_id == *superseded_by)
                    .map_or_else(String::new, |candidate| candidate.card.text.clone());
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

fn model_role_enabled(role: crate::gateway::ModelRole) -> Result<bool, TexoError> {
    let resolved = env::with(|op_env| {
        crate::gateway::resolve_role(
            role,
            &crate::gateway::RoleOverrides::default(),
            op_env.config.gateway.as_ref(),
        )
    })?;
    Ok(crate::host::grants_model_capability(Some(resolved.api_key)))
}

#[cfg(feature = "openrouter")]
fn complete_chat(
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
fn complete_chat(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(speaker: &str, text: &str, turn_no: u32) -> TurnEntry {
        TurnEntry {
            session_id: "s".to_string(),
            workspace_id: "memory".to_string(),
            speaker: speaker.to_string(),
            text: text.to_string(),
            turn_no,
            observed_at_ms: u64::from(turn_no),
        }
    }

    #[test]
    fn session_id_validation_rejects_path_escapes() {
        assert!(valid_session_id("session-1"));
        assert!(valid_session_id("A_b-9"));
        assert!(!valid_session_id(""));
        assert!(!valid_session_id("../etc/passwd"));
        assert!(!valid_session_id("a/b"));
        assert!(!valid_session_id("a.b"));
        assert!(!valid_session_id("white space"));
        assert!(!valid_session_id(&"x".repeat(MAX_SESSION_ID_LEN + 1)));
    }

    #[test]
    fn transcript_renders_user_lines_only_for_ingest() {
        let turns = vec![
            turn("user", "Deploys happen on Friday.", 1),
            turn("assistant", "Okay, deploys happen on Friday.", 2),
            turn("user", "Alice approves releases.", 3),
        ];
        assert_eq!(
            render_transcript("session-1", &turns, false),
            "# Session session-1\n\nUser: Deploys happen on Friday.\n\nUser: Alice approves releases.\n"
        );
    }

    #[test]
    fn transcript_export_includes_both_speakers() {
        let turns = vec![
            turn("user", "line one\nline two", 1),
            turn("assistant", "  spaced   out  ", 2),
        ];
        assert_eq!(
            render_transcript("s", &turns, true),
            "# Session s\n\nUser: line one line two\n\nAssistant: spaced out\n"
        );
    }
}
