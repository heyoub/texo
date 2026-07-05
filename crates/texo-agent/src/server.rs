//! axum HTTP surface: chat, memory sidebar, session end, static UI.
//!
//! All journal (BatPak) I/O runs on `spawn_blocking` worker threads through
//! the synchronous functions in [`crate::memory`] and [`crate::session`]; the
//! async side only does model HTTP and request plumbing.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::chat::{build_chat_request, build_system_prompt, complete, ChatConfig};
use crate::memory::{load_memory, MemoryClaim, MemorySnapshot};
use crate::session::{
    memorize_session, observed_at_ms, valid_session_id, SessionEndReport, SessionStore, Speaker,
    Utterance,
};

/// The single-page UI, compiled into the binary (no build step, no assets dir
/// at runtime).
const INDEX_HTML: &str = include_str!("../assets/index.html");

/// Shared server state.
pub struct AppState {
    /// texo workspace root.
    pub root: PathBuf,
    /// Optional workspace scope id (`None` = config default).
    pub workspace: Option<String>,
    /// In-memory session transcripts.
    pub sessions: SessionStore,
    /// Chat backend, `None` when no API key is configured.
    pub chat: Option<ChatConfig>,
    /// Async HTTP client for the chat model.
    pub http: reqwest::Client,
}

/// JSON API error: `{"error": message}` with a status code.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    /// An error with an explicit status.
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// A 500 carrying the full anyhow chain.
    pub fn internal(err: &anyhow::Error) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

/// `POST /api/chat` request body.
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    /// Session the message belongs to.
    pub session_id: String,
    /// The user's message.
    pub message: String,
}

/// `POST /api/chat` response body.
#[derive(Debug, Serialize)]
pub struct ChatResponse {
    /// Assistant reply.
    pub reply: String,
    /// The current claims injected into the system prompt, with receipts.
    pub memory_used: Vec<MemoryClaim>,
}

/// `POST /api/session/end` request body.
#[derive(Debug, Deserialize)]
pub struct SessionEndRequest {
    /// Session to render + ingest.
    pub session_id: String,
}

/// Build the router. Handlers are closures so the named logic below takes
/// `&AppState` (no by-value state plumbing).
pub fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(|| async { Html(INDEX_HTML) }))
        .route(
            "/api/memory",
            get(|State(state): State<Arc<AppState>>| async move {
                memory_snapshot(&state).await.map(Json)
            }),
        )
        .route(
            "/api/chat",
            post(
                |State(state): State<Arc<AppState>>, Json(request): Json<ChatRequest>| async move {
                    handle_chat(&state, request).await.map(Json)
                },
            ),
        )
        .route(
            "/api/session/end",
            post(
                |State(state): State<Arc<AppState>>,
                 Json(request): Json<SessionEndRequest>| async move {
                    handle_session_end(&state, &request.session_id).await.map(Json)
                },
            ),
        )
        .with_state(state)
}

/// Replay the journal on a blocking worker.
async fn memory_snapshot(state: &AppState) -> Result<MemorySnapshot, ApiError> {
    let root = state.root.clone();
    let workspace = state.workspace.clone();
    tokio::task::spawn_blocking(move || load_memory(&root, workspace.as_deref()))
        .await
        .map_err(|err| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("memory task failed: {err}"),
            )
        })?
        .map_err(|err| ApiError::internal(&err))
}

/// One chat turn: replay memory, ground the system prompt in it, call the
/// model, and record both utterances in the session transcript.
async fn handle_chat(state: &AppState, request: ChatRequest) -> Result<ChatResponse, ApiError> {
    if !valid_session_id(&request.session_id) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid session_id: use 1-64 ASCII letters, digits, '-' or '_'",
        ));
    }
    if request.message.trim().is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "empty message"));
    }
    let Some(chat_config) = &state.chat else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "chat is disabled: OPENROUTER_API_KEY is not set",
        ));
    };

    let memory = memory_snapshot(state).await?;
    let system_prompt = build_system_prompt(&memory);
    let history = state.sessions.history(&request.session_id);
    let body = build_chat_request(
        &chat_config.model,
        &system_prompt,
        &history,
        &request.message,
    );
    let reply = complete(&state.http, chat_config, &body)
        .await
        .map_err(|err| ApiError::new(StatusCode::BAD_GATEWAY, format!("{err:#}")))?;

    state.sessions.push(
        &request.session_id,
        Utterance {
            speaker: Speaker::User,
            text: request.message,
        },
    );
    state.sessions.push(
        &request.session_id,
        Utterance {
            speaker: Speaker::Assistant,
            text: reply.clone(),
        },
    );

    Ok(ChatResponse {
        reply,
        memory_used: memory.current,
    })
}

/// End a session: render its transcript to `sessions/<id>.md`, ingest, and
/// relate — on a blocking worker. On failure the transcript is restored so the
/// client can retry.
async fn handle_session_end(
    state: &AppState,
    session_id: &str,
) -> Result<SessionEndReport, ApiError> {
    if !valid_session_id(session_id) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid session_id: use 1-64 ASCII letters, digits, '-' or '_'",
        ));
    }
    let Some(transcript) = state.sessions.take(session_id) else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("unknown or empty session: {session_id}"),
        ));
    };

    let root = state.root.clone();
    let workspace = state.workspace.clone();
    let id = session_id.to_owned();
    let task_transcript = transcript.clone();
    let result = tokio::task::spawn_blocking(move || {
        memorize_session(
            &root,
            workspace.as_deref(),
            &id,
            &task_transcript,
            observed_at_ms(),
        )
    })
    .await
    .map_err(|err| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("session-end task failed: {err}"),
        )
    })?;

    match result {
        Ok(report) => Ok(report),
        Err(err) => {
            // Put the turns back so a retry after a transient failure (e.g.
            // extractor backend hiccup) does not lose the conversation.
            state.sessions.restore(session_id, transcript);
            Err(ApiError::internal(&err))
        }
    }
}

/// Bind and serve until the process is stopped.
pub async fn serve(state: Arc<AppState>, addr: &str) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    let local = listener.local_addr().context("reading bound address")?;
    println!("texo-agent listening on http://{local}");
    axum::serve(listener, app(state))
        .await
        .context("server error")?;
    Ok(())
}
