//! Session lane invariants for the WO-4 agent slice.

use serde_json::json;
use tempfile::TempDir;
use texo::host::TexoHost;
use texo::ops::agent::{journal_turn, read_session_turns, Speaker};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const OBSERVED_AT_MS: u64 = 1_700_000_000_000;

#[test]
fn turns_survive_crash_before_session_end() -> TestResult {
    let dir = TempDir::new()?;
    let mut host = TexoHost::open(dir.path(), "demo", OBSERVED_AT_MS)?;
    let _init = host.invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))?;
    let store = host.store();

    let _first = journal_turn(
        &store,
        "demo",
        "crash_case",
        Speaker::User,
        "Deploys happen on Friday.",
        OBSERVED_AT_MS + 1,
    )?;
    let _second = journal_turn(
        &store,
        "demo",
        "crash_case",
        Speaker::Assistant,
        "Acknowledged.",
        OBSERVED_AT_MS + 2,
    )?;
    let _third = journal_turn(
        &store,
        "demo",
        "crash_case",
        Speaker::User,
        "Decision: deploys moved to Tuesday.",
        OBSERVED_AT_MS + 3,
    )?;
    drop(store);
    drop(host);

    let mut reopened = TexoHost::open(dir.path(), "demo", OBSERVED_AT_MS + 4)?;
    let turns = read_session_turns(&reopened.store(), "crash_case")?;
    assert_eq!(turns.len(), 3);
    assert_eq!(turns[0].turn_no, 1);
    assert_eq!(turns[0].speaker, "user");
    assert_eq!(turns[1].speaker, "assistant");
    assert_eq!(turns[2].turn_no, 3);

    let memory = reopened.invoke_json("texo.agent.memory", &json!({}))?;
    assert_eq!(memory["current"].as_array().map(Vec::len), Some(0));
    assert_eq!(memory["stale"].as_array().map(Vec::len), Some(0));
    assert_eq!(memory["conflicts"].as_array().map(Vec::len), Some(0));

    let exported =
        reopened.invoke_json("texo.session.export", &json!({"session_id": "crash_case"}))?;
    let markdown = exported["markdown"]
        .as_str()
        .expect("session export returns markdown");
    assert!(markdown.contains("User: Deploys happen on Friday."));
    assert!(markdown.contains("Assistant: Acknowledged."));

    let ended = reopened.invoke_json(
        "texo.agent.session.end",
        &json!({"session_id": "crash_case", "observed_at_ms": OBSERVED_AT_MS + 5}),
    )?;
    let claims_recorded = ended["claims_recorded"]
        .as_u64()
        .expect("session end returns claim count");
    assert!(claims_recorded >= 1);
    assert_eq!(ended["relate"]["status"], "skipped");
    Ok(())
}
