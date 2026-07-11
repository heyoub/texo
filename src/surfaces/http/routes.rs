//! Inbound HTTP route handlers.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use batpak::store::{Open, Store};
use serde::Deserialize;
use serde_json::json;

use crate::error::TexoError;
use crate::host::TexoHost;
use crate::ops::agent::valid_session_id;

use super::request::{HttpRequest, Method};
use super::response::{content_type, HttpResponse};

const FALLBACK_INDEX: &str = include_str!("../../../assets/index.html");

/// Shared route state.
#[derive(Clone)]
pub struct RouteState {
    /// Workspace root.
    pub root: PathBuf,
    /// Workspace id.
    pub workspace_id: String,
    /// Shared open store for long-lived server processes.
    pub store: Option<Arc<Store<Open>>>,
    /// Whether model-backed chat should be exposed.
    pub chat_enabled: bool,
}

/// Route one parsed HTTP request.
///
/// # Errors
///
/// Returns [`TexoError`] when JSON serialization fails.
pub fn route(request: &HttpRequest, state: &RouteState) -> Result<HttpResponse, TexoError> {
    match (request.method, request.path.as_str()) {
        (Method::Get, "/api/host") => api_host(state),
        (Method::Get, "/api/memory") => api_memory(state),
        (Method::Post, "/api/host" | "/api/memory" | "/api/stream") => {
            Ok(method_not_allowed("GET"))
        }
        (Method::Post, "/api/chat") => api_chat(request, state),
        (Method::Get, "/api/chat" | "/api/session/end") => Ok(method_not_allowed("POST")),
        (Method::Post, "/api/session/end") => api_session_end(request, state),
        (Method::Get, "/api/stream") => Ok(HttpResponse::json_error(500, "sse handled by server")),
        (Method::Get, path) if !path.starts_with("/api/") => static_file(path),
        (Method::Get | Method::Post, _) if request.path.starts_with("/api/") => {
            Ok(HttpResponse::json_error(404, "not found"))
        }
        _ => {
            let mut response = HttpResponse::json_error(405, "method not allowed");
            response
                .headers
                .push(("Allow".to_string(), "GET, POST".to_string()));
            Ok(response)
        }
    }
}

fn method_not_allowed(allow: &'static str) -> HttpResponse {
    let mut response = HttpResponse::json_error(405, "method not allowed");
    response
        .headers
        .push(("Allow".to_string(), allow.to_string()));
    response
}

fn api_host(state: &RouteState) -> Result<HttpResponse, TexoError> {
    let _host = open_host(state)?;
    let interface = crate::host::fingerprint::canonical_interface(&crate::ops::catalog());
    HttpResponse::json(
        200,
        &json!({
            "fingerprint": interface.interface_fingerprint,
            "schema": interface.schema,
            "version": env!("CARGO_PKG_VERSION"),
            "workspace_id": state.workspace_id,
        }),
    )
    .map_err(TexoError::Json)
}

fn api_memory(state: &RouteState) -> Result<HttpResponse, TexoError> {
    let mut host = open_host(state)?;
    match host.invoke_json("texo.agent.memory", &json!({})) {
        Ok(value) => HttpResponse::json(200, &value).map_err(TexoError::Json),
        Err(error) => {
            HttpResponse::json(500, &json!({ "error": error.to_string() })).map_err(TexoError::Json)
        }
    }
}

fn api_chat(request: &HttpRequest, state: &RouteState) -> Result<HttpResponse, TexoError> {
    let input: ChatRequest = match serde_json::from_slice(&request.body) {
        Ok(input) => input,
        Err(error) => return Ok(HttpResponse::json_error(400, &error.to_string())),
    };
    if !valid_session_id(&input.session_id) {
        return Ok(HttpResponse::json_error(
            400,
            "invalid session_id: use 1-64 ASCII letters, digits, '-' or '_'",
        ));
    }
    if input.message.trim().is_empty() {
        return Ok(HttpResponse::json_error(400, "empty message"));
    }
    if !state.chat_enabled {
        return Ok(HttpResponse::json_error(
            503,
            "chat is disabled: TEXO_LLM_API_KEY is not set",
        ));
    }
    let mut host = open_host(state)?;
    match host.invoke_json(
        "texo.agent.chat",
        &json!({
            "session_id": input.session_id,
            "message": input.message,
            "observed_at_ms": now_ms()
        }),
    ) {
        Ok(value) => HttpResponse::json(200, &value).map_err(TexoError::Json),
        Err(TexoError::OpRuntime { denied: true, .. }) => Ok(HttpResponse::json_error(
            503,
            "chat is disabled: TEXO_LLM_API_KEY is not set",
        )),
        Err(TexoError::Model { detail }) => Ok(HttpResponse::json_error(
            502,
            &format!("model failure: {detail}"),
        )),
        Err(error) => Ok(HttpResponse::json_error(500, &error.to_string())),
    }
}

fn api_session_end(request: &HttpRequest, state: &RouteState) -> Result<HttpResponse, TexoError> {
    let input: SessionEndRequest = match serde_json::from_slice(&request.body) {
        Ok(input) => input,
        Err(error) => return Ok(HttpResponse::json_error(400, &error.to_string())),
    };
    if !valid_session_id(&input.session_id) {
        return Ok(HttpResponse::json_error(
            400,
            "invalid session_id: use 1-64 ASCII letters, digits, '-' or '_'",
        ));
    }
    let mut host = open_host(state)?;
    match host.invoke_json(
        "texo.agent.session.end",
        &json!({"session_id": input.session_id, "observed_at_ms": now_ms()}),
    ) {
        Ok(value) => HttpResponse::json(200, &value).map_err(TexoError::Json),
        Err(TexoError::MissingEntity { entity }) => Ok(HttpResponse::json_error(
            404,
            &format!(
                "unknown or empty session: {}",
                entity.trim_start_matches("session:")
            ),
        )),
        Err(TexoError::OpRuntime { detail, .. }) if detail.contains("domain.missing") => {
            Ok(HttpResponse::json_error(
                404,
                &format!("unknown or empty session: {}", input.session_id),
            ))
        }
        Err(error) => Ok(HttpResponse::json_error(500, &error.to_string())),
    }
}

fn static_file(path: &str) -> Result<HttpResponse, TexoError> {
    let ui_dir = std::env::var("TEXO_UI_DIR").unwrap_or_else(|_| "./ui/dist".to_string());
    let ui_root = PathBuf::from(ui_dir);
    if ui_root.exists() {
        let file = if path == "/" {
            ui_root.join("index.html")
        } else {
            ui_root.join(path.trim_start_matches('/'))
        };
        let root = ui_root.canonicalize()?;
        if let Ok(canonical) = file.canonicalize() {
            if canonical.starts_with(&root) && canonical.is_file() {
                let mut headers = vec![(
                    "Content-Type".to_string(),
                    content_type(&canonical).to_string(),
                )];
                if canonical.extension().and_then(std::ffi::OsStr::to_str) == Some("html") {
                    headers.push((
                        "Content-Security-Policy".to_string(),
                        "worker-src 'self' blob:; connect-src 'self'".to_string(),
                    ));
                }
                return Ok(HttpResponse::new(200, headers, std::fs::read(canonical)?));
            }
        }
        return Ok(HttpResponse::json_error(404, "not found"));
    }
    let mut response = HttpResponse::new(
        200,
        vec![(
            "Content-Type".to_string(),
            "text/html; charset=utf-8".to_string(),
        )],
        FALLBACK_INDEX.as_bytes().to_vec(),
    );
    response.headers.push((
        "Content-Security-Policy".to_string(),
        "worker-src 'self' blob:; connect-src 'self'".to_string(),
    ));
    Ok(response)
}

/// Open a host for route state.
///
/// # Errors
///
/// Returns [`TexoError`] when host composition fails.
pub fn open_host(state: &RouteState) -> Result<TexoHost, TexoError> {
    if let Some(store) = &state.store {
        TexoHost::open_with_store(
            state.root.clone(),
            state.workspace_id.clone(),
            now_ms(),
            Arc::clone(store),
        )
    } else {
        TexoHost::open(state.root.clone(), state.workspace_id.clone(), now_ms())
    }
}

fn now_ms() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    u64::try_from(millis).unwrap_or(u64::MAX)
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
    session_id: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct SessionEndRequest {
    session_id: String,
}
