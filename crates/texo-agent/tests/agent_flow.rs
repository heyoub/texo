//! PROVES the memory-agent loop over REAL BatPak stores (no test doubles, no
//! network): transcript -> markdown -> heuristic ingest -> replayed memory
//! projection -> memory-grounded system prompt, plus the HTTP surface via
//! in-process requests. The LLM chat call itself is untested by design
//! (env-gated); its request body comes from the unit-tested pure builder.

use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use tower::util::ServiceExt;

use texo_agent::bootstrap::{ensure_workspace, BootstrapOptions};
use texo_agent::chat::{build_system_prompt, OUTDATED_MEMORY_HEADER};
use texo_agent::memory::load_memory;
use texo_agent::server::{app, AppState};
use texo_agent::session::{
    memorize_session, session_doc_path, RelateOutcome, SessionStore, Speaker, Utterance,
};

const T0: u64 = 1_700_000_000_000;

fn utterance(speaker: Speaker, text: &str) -> Utterance {
    Utterance {
        speaker,
        text: text.to_owned(),
    }
}

/// Bootstrap a heuristic (offline) memory workspace and memorize two sessions
/// in which the remembered fact changes: Friday -> Tuesday. Assistant turns
/// are deliberately claim-free so only the user's facts are extracted.
fn two_session_store() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    ensure_workspace(
        dir.path(),
        &BootstrapOptions {
            workspace_id: "memory".to_owned(),
            extractor_cmd: None,
            semantics_enabled: false,
        },
    )
    .expect("bootstrap");

    let session_1 = vec![
        utterance(Speaker::User, "Deploys happen on Friday."),
        utterance(Speaker::Assistant, "Okay, remembered."),
    ];
    let report_1 = memorize_session(dir.path(), None, "session-1", &session_1, T0)
        .expect("memorize session 1");
    assert_eq!(report_1.sources_observed, 1);
    assert!(
        report_1.claims_recorded >= 1,
        "the Friday fact must be extracted: {report_1:?}"
    );
    assert_eq!(
        report_1.relate,
        RelateOutcome::Skipped {
            reason: "semantics disabled for this workspace".to_owned()
        }
    );

    let session_2 = vec![
        utterance(Speaker::User, "Deploys moved to Tuesday."),
        utterance(Speaker::Assistant, "Understood, updating."),
    ];
    let report_2 = memorize_session(dir.path(), None, "session-2", &session_2, T0 + 60_000)
        .expect("memorize session 2");
    assert!(
        report_2.ingest_supersessions >= 1,
        "the Tuesday claim must supersede the Friday claim across sessions: {report_2:?}"
    );
    dir
}

#[test]
fn cross_session_supersession_retires_the_old_fact() {
    let dir = two_session_store();
    let memory = load_memory(dir.path(), None).expect("load memory");

    // The Tuesday claim is the trusted memory; Friday was retired.
    let tuesday: Vec<_> = memory
        .current
        .iter()
        .filter(|c| c.text.contains("Deploys moved to Tuesday."))
        .collect();
    assert_eq!(tuesday.len(), 1, "current: {:?}", memory.current);
    assert!(
        !memory
            .current
            .iter()
            .any(|c| c.text.contains("Deploys happen on Friday.")),
        "the superseded fact must not be current: {:?}",
        memory.current
    );

    let friday: Vec<_> = memory
        .stale
        .iter()
        .filter(|s| s.text.contains("Deploys happen on Friday."))
        .collect();
    assert_eq!(friday.len(), 1, "stale: {:?}", memory.stale);
    assert!(
        friday[0]
            .superseded_by_text
            .contains("Deploys moved to Tuesday."),
        "stale entry must say WHAT superseded it: {:?}",
        friday[0]
    );
    assert!(memory.replayed_through_sequence > 0);
}

#[test]
fn memory_receipts_carry_char_spans_that_slice_the_transcript() {
    let dir = two_session_store();
    let memory = load_memory(dir.path(), None).expect("load memory");
    let claim = memory
        .current
        .iter()
        .find(|c| c.text.contains("Deploys moved to Tuesday."))
        .expect("tuesday claim");

    assert_eq!(claim.source_path, "session-2.md");
    // "# Session session-2" is line 1, blank line 2, the user turn line 3.
    assert_eq!(claim.line, 3);
    // The span byte range must slice the rendered transcript back to the
    // claim's source line (real receipts, not decoration).
    let doc = std::fs::read_to_string(session_doc_path(dir.path(), "session-2"))
        .expect("read transcript");
    let start = usize::try_from(claim.char_start).expect("start fits");
    let end = usize::try_from(claim.char_end).expect("end fits");
    assert!(start < end && end <= doc.len(), "span in range: {claim:?}");
    assert_eq!(&doc[start..end], "User: Deploys moved to Tuesday.");
}

#[test]
fn system_prompt_trusts_current_and_quarantines_superseded() {
    let dir = two_session_store();
    let memory = load_memory(dir.path(), None).expect("load memory");
    let prompt = build_system_prompt(&memory);

    let outdated_at = prompt
        .find(OUTDATED_MEMORY_HEADER)
        .expect("outdated section present");
    let (trusted, outdated) = prompt.split_at(outdated_at);
    assert!(
        trusted.contains("Deploys moved to Tuesday."),
        "current claim missing from the trusted section:\n{prompt}"
    );
    assert!(
        trusted.contains("[session-2.md:3]"),
        "trusted memory must carry path:line provenance:\n{prompt}"
    );
    assert!(
        !trusted.contains("Deploys happen on Friday."),
        "superseded claim leaked into the trusted section:\n{prompt}"
    );
    assert!(
        outdated.contains("Deploys happen on Friday."),
        "superseded claim must be listed as outdated:\n{prompt}"
    );
}

#[test]
fn session_end_extracts_through_the_configured_cmd_extractor() {
    // PROVES the production wiring (bootstrap extractor_cmd + semantics=true):
    // session end runs the configured extractor subprocess via texo-core's
    // `extract_via_cmd` seam — a scripted stand-in for texo-extract emitting
    // one atomic claim, exactly like texo-core's own cmd-extractor tests. The
    // heuristic supersession is suppressed (semantics owns relating) and the
    // relate pass makes zero model calls for a single claim.
    let dir = tempfile::tempdir().expect("tempdir");
    let stub = dir.path().join("stub-extract.sh");
    std::fs::write(
        &stub,
        "#!/bin/sh\nprintf '{\"line_start\": 3, \"text\": \"The user prefers dark mode.\", \
         \"subject_hint\": \"preferences\", \"predicate_hint\": \"prefers\", \
         \"object_hint\": \"dark mode\", \"confidence_ppm\": 900000, \
         \"extractor_model\": \"stub:model\", \"prompt_version\": \"stub-v1\"}\\n'\n",
    )
    .expect("write stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
            .expect("chmod stub");
    }

    ensure_workspace(
        dir.path(),
        &BootstrapOptions {
            workspace_id: "memory".to_owned(),
            extractor_cmd: Some(stub.display().to_string()),
            semantics_enabled: true,
        },
    )
    .expect("bootstrap");

    let transcript = vec![
        utterance(Speaker::User, "Please always use dark mode for me."),
        utterance(Speaker::Assistant, "Will do."),
    ];
    let report = memorize_session(dir.path(), None, "session-1", &transcript, T0)
        .expect("memorize via cmd extractor");
    assert_eq!(report.claims_recorded, 1, "{report:?}");
    assert_eq!(
        report.ingest_supersessions, 0,
        "semantics-enabled ingest must not run the keyword heuristic"
    );
    // With a single claim the relate pass judges nothing: it either skipped
    // (no key in the environment) or ran vacuously (no pairs, no model calls).
    let vacuous = matches!(
        &report.relate,
        RelateOutcome::Skipped { .. }
            | RelateOutcome::Ran {
                supersessions: 0,
                conflicts: 0,
            }
    );
    assert!(vacuous, "unexpected relate outcome: {:?}", report.relate);

    let memory = load_memory(dir.path(), None).expect("load memory");
    assert_eq!(memory.current.len(), 1);
    assert_eq!(memory.current[0].text, "The user prefers dark mode.");
    assert_eq!(memory.current[0].source_path, "session-1.md");
    assert_eq!(memory.current[0].line, 3);
}

// ---------------------------------------------------------------------------
// HTTP surface (in-process requests against the real store; no network).
// ---------------------------------------------------------------------------

fn state_for(root: &Path) -> Arc<AppState> {
    Arc::new(AppState {
        root: root.to_path_buf(),
        workspace: None,
        sessions: SessionStore::new(),
        chat: None, // no key in tests: /api/chat must refuse cleanly
        http: reqwest::Client::new(),
    })
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("json body")
}

fn post_json(uri: &str, body: &serde_json::Value) -> Request<Body> {
    Request::post(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

#[tokio::test]
async fn api_memory_serves_the_replayed_projection() {
    let dir = two_session_store();
    let router = app(state_for(dir.path()));

    let response = router
        .oneshot(
            Request::get("/api/memory")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let value = body_json(response).await;

    let current = value["current"].as_array().expect("current array");
    assert_eq!(current.len(), 1, "{value}");
    assert!(current[0]["text"]
        .as_str()
        .is_some_and(|t| t.contains("Deploys moved to Tuesday.")));
    assert_eq!(current[0]["source_path"], "session-2.md");
    assert_eq!(current[0]["line"], 3);
    assert!(current[0]["char_end"].as_u64() > current[0]["char_start"].as_u64());

    let stale = value["stale"].as_array().expect("stale array");
    assert_eq!(stale.len(), 1, "{value}");
    assert!(stale[0]["text"]
        .as_str()
        .is_some_and(|t| t.contains("Deploys happen on Friday.")));
    assert!(stale[0]["superseded_by_text"]
        .as_str()
        .is_some_and(|t| t.contains("Tuesday")));
    assert!(value["conflicts"].as_array().is_some());
}

#[tokio::test]
async fn api_session_end_memorizes_and_updates_memory() {
    let dir = two_session_store();
    let state = state_for(dir.path());

    // Simulate chat turns accumulated in this process (the /api/chat path
    // itself needs a model; the transcript store is the seam it writes to).
    state.sessions.push(
        "session-3",
        utterance(Speaker::User, "Deploys moved to Wednesday."),
    );
    state
        .sessions
        .push("session-3", utterance(Speaker::Assistant, "Okay!"));

    let router = app(Arc::clone(&state));
    let response = router
        .clone()
        .oneshot(post_json(
            "/api/session/end",
            &serde_json::json!({ "session_id": "session-3" }),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let report = body_json(response).await;
    assert_eq!(report["session_id"], "session-3");
    assert_eq!(report["doc_path"], "sessions/session-3.md");
    assert!(report["claims_recorded"].as_u64() >= Some(1), "{report}");
    assert!(
        report["ingest_supersessions"].as_u64() >= Some(1),
        "Wednesday must retire Tuesday: {report}"
    );

    // The transcript was consumed: ending again is a 404 …
    let again = router
        .clone()
        .oneshot(post_json(
            "/api/session/end",
            &serde_json::json!({ "session_id": "session-3" }),
        ))
        .await
        .expect("response");
    assert_eq!(again.status(), StatusCode::NOT_FOUND);

    // … and the NEXT session's memory shows Wednesday current, Tuesday stale.
    let memory = body_json(
        router
            .oneshot(
                Request::get("/api/memory")
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("response"),
    )
    .await;
    let texts: Vec<&str> = memory["current"]
        .as_array()
        .expect("current")
        .iter()
        .filter_map(|c| c["text"].as_str())
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("Wednesday")),
        "current: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("Tuesday")),
        "Tuesday must have been retired: {texts:?}"
    );
    let stale_texts: Vec<&str> = memory["stale"]
        .as_array()
        .expect("stale")
        .iter()
        .filter_map(|s| s["text"].as_str())
        .collect();
    assert!(
        stale_texts.iter().any(|t| t.contains("Tuesday")),
        "stale: {stale_texts:?}"
    );
}

#[tokio::test]
async fn api_guards_reject_bad_input_without_a_model() {
    let dir = two_session_store();
    let router = app(state_for(dir.path()));

    // Chat without an API key refuses with 503 and says why.
    let response = router
        .clone()
        .oneshot(post_json(
            "/api/chat",
            &serde_json::json!({ "session_id": "session-9", "message": "hi" }),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let error = body_json(response).await;
    assert!(error["error"]
        .as_str()
        .is_some_and(|e| e.contains("OPENROUTER_API_KEY")));

    // A path-escaping session id is rejected before touching disk.
    let response = router
        .clone()
        .oneshot(post_json(
            "/api/session/end",
            &serde_json::json!({ "session_id": "../evil" }),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Ending a session that never spoke is a 404, not a crash.
    let response = router
        .clone()
        .oneshot(post_json(
            "/api/session/end",
            &serde_json::json!({ "session_id": "ghost" }),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // The UI is served at the root.
    let response = router
        .oneshot(Request::get("/").body(Body::empty()).expect("req"))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let html = String::from_utf8(bytes.to_vec()).expect("utf8");
    assert!(html.contains("texo memory agent"));
}
