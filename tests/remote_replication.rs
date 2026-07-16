//! End-to-end `syncbat`/`netbat` remote replica circuit witnesses.

use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener};
use std::process::Command;
use std::sync::Arc;
use std::thread::JoinHandle;

use batpak::store::{Open, ReadOnly, Store, StoreConfig};
use serde_json::json;
use texo::config::{TexoRootConfig, WorkspaceEntry};
use texo::host::TexoHost;
use texo::replica_net::{self, Server};
use texo::surfaces::http::server::ShutdownHandle;
use texo::topology::JournalEntry;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
type ServerThread = JoinHandle<Result<netbat::TcpServeStats, texo::error::TexoError>>;
const REPLICA_TOKEN: &str = "correct horse battery staple";

fn write_topology(root: &std::path::Path, endpoint: SocketAddr) -> TestResult {
    let journals = BTreeMap::from([
        (
            "canonical".to_string(),
            JournalEntry::canonical(".texo/store"),
        ),
        ("other".to_string(), JournalEntry::canonical(".texo/other")),
        (
            "remote".to_string(),
            JournalEntry::remote_replica(
                ".texo/replicas/remote",
                "canonical",
                endpoint.to_string(),
                "TEXO_TEST_REPLICA_TOKEN",
            ),
        ),
        (
            "wrong-source".to_string(),
            JournalEntry::remote_replica(
                ".texo/replicas/wrong-source",
                "other",
                endpoint.to_string(),
                "TEXO_TEST_REPLICA_TOKEN",
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
    std::fs::write(path, text)?;
    let mut host = TexoHost::open_journal(root, "demo", "canonical", at)?;
    let _ = host.invoke_json(
        "texo.ingest.run",
        &json!({"path": file, "dry_run": false, "observed_at_ms": at}),
    )?;
    Ok(())
}

fn start_server(
    root: &std::path::Path,
    listener: TcpListener,
) -> TestResult<(ShutdownHandle, ServerThread)> {
    let store = Arc::new(Store::<Open>::open(StoreConfig::new(
        root.join(".texo/store"),
    ))?);
    let server = Server {
        listener,
        store,
        workspace_id: "demo".to_string(),
        journal_id: "canonical".to_string(),
        token: "correct horse battery staple".to_string(),
    };
    let shutdown = ShutdownHandle::new();
    let thread_shutdown = shutdown.clone();
    let handle = std::thread::Builder::new()
        .name("texo-remote-replica-test".to_string())
        .spawn(move || replica_net::serve(&server, &thread_shutdown))?;
    Ok((shutdown, handle))
}

fn stop_server(shutdown: &ShutdownHandle, handle: ServerThread) -> TestResult {
    shutdown.shutdown();
    let _stats = handle
        .join()
        .map_err(|_| "replica server thread panicked")??;
    Ok(())
}

fn replica_command(
    root: &std::path::Path,
    token: &str,
    action: &str,
    replica: &str,
) -> TestResult<std::process::Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_texo"))
        .arg("--root")
        .arg(root)
        .arg("--workspace")
        .arg("demo")
        .arg("replica")
        .arg(action)
        .arg(replica)
        .arg("--json")
        .env("TEXO_TEST_REPLICA_TOKEN", token)
        .output()?)
}

fn claims(root: &std::path::Path, journal: &str) -> TestResult<serde_json::Value> {
    let mut host = TexoHost::open_journal(root, "demo", journal, 100)?;
    let mut claims =
        host.invoke_json("texo.claims.list", &json!({"subject": null}))?["claims"].clone();
    if let Some(rows) = claims.as_array_mut() {
        for row in rows {
            if let Some(object) = row.as_object_mut() {
                object.remove("receipt");
            }
        }
    }
    Ok(claims)
}

fn replica_event_count(root: &std::path::Path) -> TestResult<usize> {
    let store =
        Store::<ReadOnly>::open_read_only(StoreConfig::new(root.join(".texo/replicas/remote")))?;
    Ok(store.stats().event_count)
}

fn assert_remote_rejections(root: &std::path::Path) -> TestResult {
    let denied = replica_command(root, "wrong", "bootstrap", "remote")?;
    assert!(!denied.status.success());
    assert!(!root.join(".texo/replicas/remote").exists());
    let wrong_source = replica_command(root, REPLICA_TOKEN, "bootstrap", "wrong-source")?;
    assert!(!wrong_source.status.success());
    assert!(!root.join(".texo/replicas/wrong-source").exists());
    Ok(())
}

fn bootstrap_remote_replica(
    root: &std::path::Path,
    shutdown: &ShutdownHandle,
    handle: ServerThread,
) -> TestResult<Vec<u8>> {
    let initial = replica_command(root, REPLICA_TOKEN, "bootstrap", "remote")?;
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    stop_server(shutdown, handle)?;
    assert_eq!(claims(root, "canonical")?, claims(root, "remote")?);
    Ok(std::fs::read(
        root.join(".texo/replication/demo/remote/cursor.msgpack"),
    )?)
}

fn assert_follow_resumes_and_deduplicates(
    root: &std::path::Path,
    endpoint: SocketAddr,
    stale_cursor: &[u8],
) -> TestResult {
    ingest(
        root,
        "docs/two.md",
        "Decision: deploys moved to Tuesday.\n",
        2,
    )?;
    let listener = TcpListener::bind(endpoint)?;
    let (shutdown, handle) = start_server(root, listener)?;
    let advanced = replica_command(root, REPLICA_TOKEN, "follow", "remote")?;
    assert!(
        advanced.status.success(),
        "{}",
        String::from_utf8_lossy(&advanced.stderr)
    );
    std::fs::write(
        root.join(".texo/replication/demo/remote/cursor.msgpack"),
        stale_cursor,
    )?;
    let replay = replica_command(root, REPLICA_TOKEN, "follow", "remote")?;
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let replay: serde_json::Value = serde_json::from_slice(&replay.stdout)?;
    assert_eq!(replay["imported"], 0);
    assert!(replay["deduplicated"]
        .as_u64()
        .is_some_and(|count| count > 0));
    let before_noop = replica_event_count(root)?;
    let no_op = replica_command(root, REPLICA_TOKEN, "follow", "remote")?;
    assert!(no_op.status.success());
    assert_eq!(replica_event_count(root)?, before_noop);
    stop_server(&shutdown, handle)?;
    assert_eq!(claims(root, "canonical")?, claims(root, "remote")?);
    Ok(())
}

#[test]
fn remote_replica_authenticates_binds_resumes_and_deduplicates() -> TestResult {
    let root = tempfile::tempdir()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = listener.local_addr()?;
    write_topology(root.path(), endpoint)?;
    ingest(
        root.path(),
        "docs/one.md",
        "Decision: deploys happen on Friday.\n",
        1,
    )?;
    let (shutdown, handle) = start_server(root.path(), listener)?;
    assert_remote_rejections(root.path())?;
    let stale_cursor = bootstrap_remote_replica(root.path(), &shutdown, handle)?;
    assert_follow_resumes_and_deduplicates(root.path(), endpoint, &stale_cursor)
}
