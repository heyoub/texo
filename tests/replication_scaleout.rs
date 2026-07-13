//! Exact-fork and resumable imported-read-model integration witnesses.

use std::collections::BTreeMap;

use batpak::store::{ReadOnly, Store, StoreConfig};
use serde_json::json;
use texo::config::{TexoRootConfig, WorkspaceEntry};
use texo::events::payloads::ReplicaBatchMaterializedV1;
use texo::host::TexoHost;
use texo::replication::{ReplicaCursor, ReplicaReport};
use texo::topology::{JournalEntry, ReplicaMode};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn write_topology(root: &std::path::Path) -> TestResult {
    let journals = BTreeMap::from([
        (
            "canonical".to_string(),
            JournalEntry::canonical(".texo/store"),
        ),
        (
            "snapshot".to_string(),
            JournalEntry::replica(
                ".texo/replicas/snapshot",
                "canonical",
                ReplicaMode::ExactFork,
            ),
        ),
        (
            "agent".to_string(),
            JournalEntry::replica(
                ".texo/replicas/agent",
                "canonical",
                ReplicaMode::ImportedReadModel,
            ),
        ),
    ]);
    TexoRootConfig {
        default_workspace: "demo".to_string(),
        workspaces: BTreeMap::from([(
            "demo".to_string(),
            WorkspaceEntry {
                primary_journal: "canonical".to_string(),
                journals,
                docs_glob: "docs/**/*.md".to_string(),
                extractor_cmd: None,
                semantics: None,
            },
        )]),
        gateway: None,
    }
    .save(&root.join(".texo/config.toml"))?;
    Ok(())
}

fn ingest(root: &std::path::Path, file: &str, text: &str, at: u64) -> TestResult {
    let path = root.join(file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, text)?;
    let mut host = TexoHost::open_journal(root, "demo", "canonical", at)?;
    let _ = host.invoke_json(
        "texo.ingest.run",
        &json!({"path": file, "dry_run": false, "observed_at_ms": at}),
    )?;
    Ok(())
}

fn claims(root: &std::path::Path, journal: &str) -> TestResult<serde_json::Value> {
    let mut host = TexoHost::open_journal(root, "demo", journal, 99)?;
    let output = host.invoke_json("texo.claims.list", &json!({"subject": null}))?;
    Ok(output["claims"].clone())
}

fn semantic_claims(root: &std::path::Path, journal: &str) -> TestResult<serde_json::Value> {
    let mut value = claims(root, journal)?;
    if let Some(rows) = value.as_array_mut() {
        for row in rows {
            if let Some(object) = row.as_object_mut() {
                object.remove("receipt");
            }
        }
    }
    Ok(value)
}

fn replica_event_count(root: &std::path::Path, journal: &str) -> TestResult<usize> {
    let config = TexoRootConfig::load(&root.join(".texo/config.toml"))?;
    let (workspace, _) = config.resolve_journal(Some("demo"), Some(journal))?;
    let store =
        Store::<ReadOnly>::open_read_only(StoreConfig::new(workspace.store_path_buf(root)))?;
    Ok(store.stats().event_count)
}

fn ledger_source_count(root: &std::path::Path) -> TestResult<usize> {
    let config = TexoRootConfig::load(&root.join(".texo/config.toml"))?;
    let (workspace, _) = config.resolve_journal(Some("demo"), Some("agent"))?;
    let store =
        Store::<ReadOnly>::open_read_only(StoreConfig::new(workspace.store_path_buf(root)))?;
    let mut count = 0;
    for entry in store.by_entity("replica-ledger:demo:agent") {
        let raw = store.read_raw(entry.event_id())?;
        let payload: ReplicaBatchMaterializedV1 = batpak::encoding::from_bytes(&raw.event.payload)?;
        count += payload.events.len();
    }
    Ok(count)
}

#[test]
fn exact_fork_preserves_point_in_time_identity_and_refuses_follow() -> TestResult {
    let root = tempfile::tempdir()?;
    write_topology(root.path())?;
    {
        let mut canonical = TexoHost::open_journal(root.path(), "demo", "canonical", 1)?;
        let _ = canonical.invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))?;
    }
    ingest(
        root.path(),
        "docs/one.md",
        "Decision: deploys happen on Friday.\n",
        2,
    )?;

    let report = texo::replication::bootstrap(root.path(), Some("demo"), "snapshot")?;
    let ReplicaReport::ExactFork { evidence, .. } = report else {
        return Err("expected exact fork report".into());
    };
    assert_eq!(evidence.source_frontier, evidence.replica_frontier);
    assert_eq!(
        claims(root.path(), "canonical")?,
        claims(root.path(), "snapshot")?
    );

    let error = texo::replication::follow_once(root.path(), Some("demo"), "snapshot")
        .expect_err("exact point-in-time forks cannot silently become import replicas");
    assert_eq!(error.code(), "replication.mode");
    Ok(())
}

#[test]
fn imported_read_model_resumes_and_replays_after_stale_cursor_without_duplicates() -> TestResult {
    let root = tempfile::tempdir()?;
    write_topology(root.path())?;
    {
        let mut canonical = TexoHost::open_journal(root.path(), "demo", "canonical", 1)?;
        let _ = canonical.invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))?;
    }
    ingest(
        root.path(),
        "docs/one.md",
        "Decision: deploys happen on Friday.\n",
        2,
    )?;
    let initial = texo::replication::bootstrap(root.path(), Some("demo"), "agent")?;
    let ReplicaReport::ImportedReadModel { cursor, .. } = initial else {
        return Err("expected imported read model report".into());
    };
    let cursor_path = root
        .path()
        .join(".texo/replication/demo/agent/cursor.msgpack");
    let stale_cursor_bytes = std::fs::read(&cursor_path)?;

    ingest(
        root.path(),
        "docs/two.md",
        "Decision: deploys moved to Tuesday.\n",
        3,
    )?;
    let advanced = texo::replication::follow_once(root.path(), Some("demo"), "agent")?;
    let ReplicaReport::ImportedReadModel {
        imported,
        cursor: advanced_cursor,
        ..
    } = advanced
    else {
        return Err("expected imported read model report".into());
    };
    assert!(imported > 0);
    assert!(advanced_cursor.source_high_watermark > cursor.source_high_watermark);
    // Crash after BatPak committed the import but before Texo published the new
    // cursor: restoring the stale cursor must re-read safely and deduplicate.
    assert!(ledger_source_count(root.path())? > 0);
    std::fs::write(&cursor_path, stale_cursor_bytes)?;
    let before_replay = replica_event_count(root.path(), "agent")?;
    let replay = texo::replication::follow_once(root.path(), Some("demo"), "agent")?;
    let after_replay = replica_event_count(root.path(), "agent")?;
    let ReplicaReport::ImportedReadModel {
        imported,
        deduplicated,
        cursor: replayed_cursor,
        ..
    } = replay
    else {
        return Err("expected imported read model report".into());
    };
    assert_eq!(
        after_replay,
        before_replay + 1,
        "only BatPak's mutable-open lifecycle event may advance the destination"
    );
    assert_eq!(imported, 0);
    assert!(deduplicated > 0);
    assert_eq!(
        replayed_cursor.source_high_watermark,
        advanced_cursor.source_high_watermark
    );
    assert_eq!(
        replayed_cursor.source_anchor_event_id_hex,
        advanced_cursor.source_anchor_event_id_hex
    );
    assert_eq!(
        replayed_cursor.replica_frontier,
        advanced_cursor.replica_frontier + 1,
        "BatPak records one mutable-open lifecycle event per follower run"
    );
    let before_noop = replica_event_count(root.path(), "agent")?;
    let no_op = texo::replication::follow_once(root.path(), Some("demo"), "agent")?;
    let ReplicaReport::ImportedReadModel { imported, .. } = no_op else {
        return Err("expected imported read model report".into());
    };
    assert_eq!(imported, 0);
    assert_eq!(replica_event_count(root.path(), "agent")?, before_noop);
    assert_eq!(
        semantic_claims(root.path(), "canonical")?,
        semantic_claims(root.path(), "agent")?
    );
    Ok(())
}

#[test]
fn source_anchor_swap_is_refused_before_any_replica_append() -> TestResult {
    let root = tempfile::tempdir()?;
    write_topology(root.path())?;
    {
        let mut canonical = TexoHost::open_journal(root.path(), "demo", "canonical", 1)?;
        let _ = canonical.invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))?;
    }
    let _ = texo::replication::bootstrap(root.path(), Some("demo"), "agent")?;
    let cursor_path = root
        .path()
        .join(".texo/replication/demo/agent/cursor.msgpack");
    let bytes = std::fs::read(&cursor_path)?;
    let mut cursor: ReplicaCursor = batpak::encoding::from_bytes(&bytes)?;
    cursor.source_anchor_event_id_hex = Some("00".repeat(16));
    std::fs::write(&cursor_path, batpak::encoding::to_bytes(&cursor)?)?;
    let before = replica_event_count(root.path(), "agent")?;

    let error = texo::replication::follow_once(root.path(), Some("demo"), "agent")
        .expect_err("source anchor mismatch must fail closed");
    assert_eq!(error.code(), "replication.anchor");
    let after = replica_event_count(root.path(), "agent")?;
    assert_eq!(after, before);
    Ok(())
}
