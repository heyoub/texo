//! Scale-out replica circuits composed from BatPak lifecycle and import APIs.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use batpak::coordinate::Coordinate;
use batpak::event::EventPayload;
use batpak::id::IdempotencyKey;
use batpak::store::{
    AppendOptions, BatchAppendItem, CausationRef, Open, ReadOnly, Store, StoreConfig, StoreState,
};
use serde::{Deserialize, Serialize};

use crate::compat::batpak as substrate;
use crate::config::{TexoRootConfig, WorkspaceConfig};
use crate::error::{Committed, ReplicationFailureKind, TexoError};
use crate::events::payloads::{ReplicaBatchMaterializedV1, ReplicaSourceEventV1};
use crate::topology::{JournalRole, ReplicaMode, ResolvedJournal};

const STATE_SCHEMA_VERSION: u32 = 1;

/// Durable operational cursor for one imported read model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicaCursor {
    /// Cursor schema.
    pub schema_version: u32,
    /// Logical workspace scope.
    pub workspace_id: String,
    /// Stable source journal id.
    pub source_journal: String,
    /// Stable destination journal id.
    pub replica_journal: String,
    /// Stable import namespace used in destination idempotency keys.
    pub source_namespace: String,
    /// Highest source sequence fully observed by the last successful call.
    pub source_high_watermark: Option<u64>,
    /// Event id at the source cursor, used to reject truncation/store swap.
    pub source_anchor_event_id_hex: Option<String>,
    /// Destination visible frontier after the last successful call.
    pub replica_frontier: u64,
    /// Event id at the destination frontier after the last successful call.
    pub replica_anchor_event_id_hex: Option<String>,
}

/// Exact point-in-time fork evidence retained outside the copied store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExactForkEvidence {
    /// Evidence schema.
    pub schema_version: u32,
    /// Logical workspace scope.
    pub workspace_id: String,
    /// Stable source journal id.
    pub source_journal: String,
    /// Stable destination journal id.
    pub replica_journal: String,
    /// `BatPak` structural fork identity.
    pub fork_id_hex: String,
    /// Canonical `BatPak` fork report hash.
    pub report_hash_hex: String,
    /// Source visible frontier at the fork boundary.
    pub source_frontier: u64,
    /// Fork visible frontier after reopen.
    pub replica_frontier: u64,
    /// Number of events verified in the reopened fork.
    pub events_verified: usize,
}

/// Machine-readable result of one replica operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ReplicaReport {
    /// Identity-preserving point-in-time clone.
    ExactFork {
        /// Persisted structural evidence.
        evidence: ExactForkEvidence,
        /// Workspace-relative evidence artifact.
        evidence_path: String,
    },
    /// Destination-local imported read model.
    ImportedReadModel {
        /// Number of source events newly materialized.
        imported: u64,
        /// Number already present by deterministic import identity.
        deduplicated: u64,
        /// Number of substrate-reserved events intentionally omitted.
        skipped_reserved: u64,
        /// Replica-ledger events omitted when following another replica.
        skipped_operational: u64,
        /// Cursor persisted after verification.
        cursor: ReplicaCursor,
        /// Workspace-relative cursor artifact.
        cursor_path: String,
    },
}

struct Circuit {
    workspace: WorkspaceConfig,
    source: ResolvedJournal,
    replica: ResolvedJournal,
    source_path: PathBuf,
    replica_path: PathBuf,
}

/// Bootstrap the configured replica according to its frozen mode.
///
/// Exact forks preserve event identities at one point in time. Imported read
/// models receive destination-local event ids plus an atomic replica ledger.
/// Existing destination bytes are never cleared by Texo.
///
/// # Errors
/// Fails closed on malformed topology, non-fresh destinations, `BatPak` errors,
/// verification failures, or evidence persistence failures.
pub fn bootstrap(
    root: &Path,
    workspace_id: Option<&str>,
    replica_id: &str,
) -> Result<ReplicaReport, TexoError> {
    let circuit = resolve_circuit(root, workspace_id, replica_id)?;
    ensure_fresh_destination(&circuit.replica_path)?;
    match circuit.replica.replica_mode {
        Some(ReplicaMode::ExactFork) => bootstrap_exact(root, &circuit),
        Some(ReplicaMode::ImportedReadModel) => bootstrap_imported(root, &circuit),
        None => Err(replication_error(
            ReplicationFailureKind::InvalidTopology,
            Committed::No,
            "replica declaration has no materialization mode",
        )),
    }
}

/// Resume one configured imported read model from its verified source cursor.
///
/// # Errors
/// Fails closed when the circuit is not an imported read model, its cursor is
/// missing/mismatched, the source anchor changed, or import/verification fails.
pub fn follow_once(
    root: &Path,
    workspace_id: Option<&str>,
    replica_id: &str,
) -> Result<ReplicaReport, TexoError> {
    let circuit = resolve_circuit(root, workspace_id, replica_id)?;
    if circuit.replica.replica_mode != Some(ReplicaMode::ImportedReadModel) {
        return Err(replication_error(
            ReplicationFailureKind::ModeMismatch,
            Committed::No,
            "only imported_read_model replicas can follow a changing source",
        ));
    }
    let cursor = load_cursor(root, &circuit.workspace.workspace_id, replica_id)?;
    validate_cursor_binding(&cursor, &circuit)?;
    import_from_cursor(root, &circuit, Some(&cursor))
}

fn bootstrap_exact(root: &Path, circuit: &Circuit) -> Result<ReplicaReport, TexoError> {
    let source = open_store(&circuit.source_path, Committed::No)?;
    let source_frontier = frontier(&source);
    let report = substrate::exact_fork(&source, &circuit.replica_path).map_err(|error| {
        replication_error(
            ReplicationFailureKind::Substrate,
            Committed::Unknown,
            format!("exact fork failed: {error}"),
        )
    })?;
    let forked = open_read_only_store(&circuit.replica_path, Committed::Yes)?;
    let events_verified = substrate::verify_intact(&forked).map_err(|error| {
        replication_error(
            ReplicationFailureKind::Verification,
            Committed::Yes,
            error.to_string(),
        )
    })?;
    let replica_frontier = frontier(&forked);
    if source_frontier != replica_frontier {
        return Err(replication_error(
            ReplicationFailureKind::Verification,
            Committed::Yes,
            format!(
                "fork frontier {replica_frontier} does not match source frontier {source_frontier}"
            ),
        ));
    }
    let evidence = ExactForkEvidence {
        schema_version: STATE_SCHEMA_VERSION,
        workspace_id: circuit.workspace.workspace_id.clone(),
        source_journal: circuit.source.id.to_string(),
        replica_journal: circuit.replica.id.to_string(),
        fork_id_hex: hex_lower(&report.body.fork_id),
        report_hash_hex: hex_lower(&report.body_hash),
        source_frontier,
        replica_frontier,
        events_verified,
    };
    let path = evidence_path(
        root,
        &circuit.workspace.workspace_id,
        circuit.replica.id.as_str(),
        "fork-evidence.msgpack",
    );
    persist(&path, &evidence, Committed::Yes)?;
    Ok(ReplicaReport::ExactFork {
        evidence,
        evidence_path: display_relative(root, &path),
    })
}

fn bootstrap_imported(root: &Path, circuit: &Circuit) -> Result<ReplicaReport, TexoError> {
    import_from_cursor(root, circuit, None)
}

fn import_from_cursor(
    root: &Path,
    circuit: &Circuit,
    previous: Option<&ReplicaCursor>,
) -> Result<ReplicaReport, TexoError> {
    let source = open_read_only_store(&circuit.source_path, Committed::No)?;
    if let Some(cursor) = previous.as_ref() {
        validate_source_anchor(&source, cursor)?;
    }
    let destination = open_store(&circuit.replica_path, Committed::No)?;
    let namespace = source_namespace(&circuit.workspace.workspace_id, circuit.source.id.as_str());
    let mut after = previous
        .as_ref()
        .and_then(|cursor| cursor.source_high_watermark);
    let source_ceiling = frontier(&source);
    let represented = represented_source_events(
        &destination,
        &circuit.workspace.workspace_id,
        circuit.replica.id.as_str(),
        &namespace,
    )?;
    let mut imported = 0_u64;
    let mut deduplicated = 0_u64;
    let mut skipped_reserved = 0_u64;
    let mut skipped_operational = 0_u64;
    loop {
        let page = substrate::read_import_page(
            &source,
            after,
            source_ceiling,
            &represented,
            ReplicaBatchMaterializedV1::KIND,
        )
        .map_err(|error| {
            replication_error(
                ReplicationFailureKind::Substrate,
                Committed::Unknown,
                format!("source page read failed: {error}"),
            )
        })?;
        deduplicated = deduplicated.saturating_add(page.deduplicated);
        skipped_reserved = skipped_reserved.saturating_add(page.skipped_reserved);
        skipped_operational = skipped_operational.saturating_add(page.skipped_operational);
        let next = page.high_watermark;
        let has_more = page.has_more;
        if !page.events.is_empty() {
            let ledger = replica_ledger_item(circuit, &namespace, &page.events)?;
            let count =
                substrate::append_with_ledger(&destination, &namespace, page.events, ledger)
                    .map_err(|error| {
                        replication_error(
                            ReplicationFailureKind::Substrate,
                            Committed::Unknown,
                            format!("atomic replica batch failed: {error}"),
                        )
                    })?;
            imported = imported.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
        }
        if next == after || !has_more {
            after = next;
            break;
        }
        after = next;
    }
    verify_replica(&destination)?;
    let source_high_watermark = after;
    let source_anchor_event_id_hex =
        source_high_watermark.and_then(|sequence| substrate::event_id_at(&source, sequence));
    if source_high_watermark.is_some() && source_anchor_event_id_hex.is_none() {
        return Err(replication_error(
            ReplicationFailureKind::AnchorMismatch,
            Committed::Partial,
            "source high watermark has no readable anchor event",
        ));
    }
    let replica_frontier = frontier(&destination);
    let cursor = ReplicaCursor {
        schema_version: STATE_SCHEMA_VERSION,
        workspace_id: circuit.workspace.workspace_id.clone(),
        source_journal: circuit.source.id.to_string(),
        replica_journal: circuit.replica.id.to_string(),
        source_namespace: namespace,
        source_high_watermark,
        source_anchor_event_id_hex,
        replica_frontier,
        replica_anchor_event_id_hex: substrate::event_id_at(&destination, replica_frontier),
    };
    let path = cursor_path(
        root,
        &circuit.workspace.workspace_id,
        circuit.replica.id.as_str(),
    );
    persist(&path, &cursor, Committed::Partial)?;
    Ok(ReplicaReport::ImportedReadModel {
        imported,
        deduplicated,
        skipped_reserved,
        skipped_operational,
        cursor,
        cursor_path: display_relative(root, &path),
    })
}

fn verify_replica(destination: &Store<Open>) -> Result<(), TexoError> {
    substrate::verify_intact(destination)
        .map(|_| ())
        .map_err(|error| {
            replication_error(
                ReplicationFailureKind::Verification,
                Committed::Partial,
                error.to_string(),
            )
        })
}

fn replica_ledger_item(
    circuit: &Circuit,
    namespace: &str,
    events: &[substrate::ImportEvent],
) -> Result<BatchAppendItem, TexoError> {
    let entries = events
        .iter()
        .map(|event| ReplicaSourceEventV1 {
            source_event_id_hex: event.source.event_id_hex.clone(),
            source_global_sequence: event.source.global_sequence,
            source_kind: event.source.kind_raw,
            source_content_hash_hex: hex_lower(&event.source.content_hash),
        })
        .collect::<Vec<_>>();
    let first = entries.first().map_or_else(
        || "0".to_string(),
        |entry| entry.source_global_sequence.to_string(),
    );
    let last = entries.last().map_or_else(
        || "0".to_string(),
        |entry| entry.source_global_sequence.to_string(),
    );
    let payload = ReplicaBatchMaterializedV1 {
        workspace_id: circuit.workspace.workspace_id.clone(),
        source_journal: circuit.source.id.to_string(),
        replica_journal: circuit.replica.id.to_string(),
        source_namespace: namespace.to_string(),
        events: entries,
    };
    let coordinate =
        replica_ledger_coordinate(&circuit.workspace.workspace_id, circuit.replica.id.as_str())?;
    let options = AppendOptions::new().with_idempotency(IdempotencyKey::for_operation(
        "texo.replica.batch.v1",
        &[
            &circuit.workspace.workspace_id,
            circuit.source.id.as_str(),
            circuit.replica.id.as_str(),
            &first,
            &last,
        ],
    ));
    BatchAppendItem::typed(coordinate, &payload, options, CausationRef::None).map_err(|error| {
        replication_error(
            ReplicationFailureKind::Substrate,
            Committed::No,
            format!("replica ledger encoding failed: {error}"),
        )
    })
}

fn represented_source_events(
    destination: &Store<Open>,
    workspace: &str,
    replica: &str,
    namespace: &str,
) -> Result<BTreeSet<String>, TexoError> {
    let coordinate = replica_ledger_coordinate(workspace, replica)?;
    let mut represented = BTreeSet::new();
    for entry in destination.by_entity(coordinate.entity()) {
        if entry.event_kind() != ReplicaBatchMaterializedV1::KIND {
            continue;
        }
        let raw = destination.read_raw(entry.event_id()).map_err(|error| {
            replication_error(
                ReplicationFailureKind::Verification,
                Committed::No,
                format!("replica ledger read failed: {error}"),
            )
        })?;
        let payload: ReplicaBatchMaterializedV1 = batpak::encoding::from_bytes(&raw.event.payload)
            .map_err(|error| {
                replication_error(
                    ReplicationFailureKind::Verification,
                    Committed::No,
                    format!("replica ledger decode failed: {error}"),
                )
            })?;
        if payload.workspace_id != workspace
            || payload.replica_journal != replica
            || payload.source_namespace != namespace
        {
            return Err(replication_error(
                ReplicationFailureKind::Verification,
                Committed::No,
                "replica ledger binding does not match the configured circuit",
            ));
        }
        represented.extend(
            payload
                .events
                .into_iter()
                .map(|source| source.source_event_id_hex),
        );
    }
    Ok(represented)
}

fn replica_ledger_coordinate(workspace: &str, replica: &str) -> Result<Coordinate, TexoError> {
    Coordinate::new(
        format!("replica-ledger:{workspace}:{replica}"),
        format!("replication:{workspace}"),
    )
    .map_err(TexoError::from)
}

fn resolve_circuit(
    root: &Path,
    workspace_id: Option<&str>,
    replica_id: &str,
) -> Result<Circuit, TexoError> {
    let config_path = root.join(".texo/config.toml");
    let root_config = TexoRootConfig::load(&config_path).map_err(|error| {
        replication_error(
            ReplicationFailureKind::InvalidTopology,
            Committed::No,
            error.to_string(),
        )
    })?;
    let (workspace, replica) = root_config
        .resolve_journal(workspace_id, Some(replica_id))
        .map_err(|error| {
            replication_error(
                ReplicationFailureKind::InvalidTopology,
                Committed::No,
                error.to_string(),
            )
        })?;
    if replica.role != JournalRole::Replica {
        return Err(replication_error(
            ReplicationFailureKind::InvalidTopology,
            Committed::No,
            format!("journal `{replica_id}` is not a replica"),
        ));
    }
    let source_id = replica.source_journal.as_ref().ok_or_else(|| {
        replication_error(
            ReplicationFailureKind::InvalidTopology,
            Committed::No,
            "replica has no source journal",
        )
    })?;
    let (source_config, source) = root_config
        .resolve_journal(Some(&workspace.workspace_id), Some(source_id.as_str()))
        .map_err(|error| {
            replication_error(
                ReplicationFailureKind::InvalidTopology,
                Committed::No,
                error.to_string(),
            )
        })?;
    let source_path = source_config.store_path_buf(root);
    let replica_path = workspace.store_path_buf(root);
    if normalize_path(&source_path)? == normalize_path(&replica_path)? {
        return Err(replication_error(
            ReplicationFailureKind::InvalidTopology,
            Committed::No,
            "source and replica resolve to the same physical path",
        ));
    }
    Ok(Circuit {
        workspace,
        source,
        replica,
        source_path,
        replica_path,
    })
}

fn ensure_fresh_destination(path: &Path) -> Result<(), TexoError> {
    if !path.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(replication_error(
            ReplicationFailureKind::DestinationNotFresh,
            Committed::No,
            format!("destination {} is not a fresh directory", path.display()),
        ));
    }
    let mut entries = fs::read_dir(path)?;
    if entries.next().transpose()?.is_some() {
        return Err(replication_error(
            ReplicationFailureKind::DestinationNotFresh,
            Committed::No,
            format!("destination {} is not empty", path.display()),
        ));
    }
    Ok(())
}

fn open_store(path: &Path, committed: Committed) -> Result<Store<Open>, TexoError> {
    Store::open(StoreConfig::new(path)).map_err(|error| {
        replication_error(
            ReplicationFailureKind::Substrate,
            committed,
            format!("store {}: {error}", path.display()),
        )
    })
}

fn open_read_only_store(path: &Path, committed: Committed) -> Result<Store<ReadOnly>, TexoError> {
    Store::<ReadOnly>::open_read_only(StoreConfig::new(path)).map_err(|error| {
        replication_error(
            ReplicationFailureKind::Substrate,
            committed,
            format!("read-only store {}: {error}", path.display()),
        )
    })
}

fn validate_cursor_binding(cursor: &ReplicaCursor, circuit: &Circuit) -> Result<(), TexoError> {
    let expected_namespace =
        source_namespace(&circuit.workspace.workspace_id, circuit.source.id.as_str());
    if cursor.schema_version != STATE_SCHEMA_VERSION
        || cursor.workspace_id != circuit.workspace.workspace_id
        || cursor.source_journal != circuit.source.id.as_str()
        || cursor.replica_journal != circuit.replica.id.as_str()
        || cursor.source_namespace != expected_namespace
    {
        return Err(replication_error(
            ReplicationFailureKind::Evidence,
            Committed::No,
            "replica cursor does not match the configured circuit",
        ));
    }
    Ok(())
}

fn validate_source_anchor<S: StoreState>(
    source: &Store<S>,
    cursor: &ReplicaCursor,
) -> Result<(), TexoError> {
    let Some(sequence) = cursor.source_high_watermark else {
        return Ok(());
    };
    let actual = substrate::event_id_at(source, sequence);
    if actual != cursor.source_anchor_event_id_hex {
        return Err(replication_error(
            ReplicationFailureKind::AnchorMismatch,
            Committed::No,
            format!("source anchor changed at global sequence {sequence}"),
        ));
    }
    Ok(())
}

fn load_cursor(root: &Path, workspace: &str, replica: &str) -> Result<ReplicaCursor, TexoError> {
    let path = cursor_path(root, workspace, replica);
    let bytes = fs::read(&path).map_err(|error| {
        replication_error(
            ReplicationFailureKind::Evidence,
            Committed::No,
            format!("read {}: {error}", path.display()),
        )
    })?;
    batpak::encoding::from_bytes(&bytes).map_err(|error| {
        replication_error(
            ReplicationFailureKind::Evidence,
            Committed::No,
            format!("decode {}: {error}", path.display()),
        )
    })
}

fn persist<T: Serialize>(path: &Path, value: &T, committed: Committed) -> Result<(), TexoError> {
    let bytes = batpak::encoding::to_bytes(value).map_err(|error| {
        replication_error(
            ReplicationFailureKind::Evidence,
            committed,
            format!("encode {}: {error}", path.display()),
        )
    })?;
    let parent = path.parent().ok_or_else(|| {
        replication_error(
            ReplicationFailureKind::Evidence,
            committed,
            "replication evidence path has no parent",
        )
    })?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let result = (|| -> Result<(), std::io::Error> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            directory.sync_all()?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ignored = fs::remove_file(&temporary);
        return Err(replication_error(
            ReplicationFailureKind::Evidence,
            committed,
            format!("persist {}: {error}", path.display()),
        ));
    }
    Ok(())
}

fn cursor_path(root: &Path, workspace: &str, replica: &str) -> PathBuf {
    evidence_path(root, workspace, replica, "cursor.msgpack")
}

fn evidence_path(root: &Path, workspace: &str, replica: &str, file: &str) -> PathBuf {
    root.join(".texo")
        .join("replication")
        .join(workspace)
        .join(replica)
        .join(file)
}

fn source_namespace(workspace: &str, source: &str) -> String {
    format!("texo.replica.v1:{workspace}:{source}")
}

fn frontier<S: StoreState>(store: &Store<S>) -> u64 {
    store.frontier().visible_hlc.global_sequence
}

fn normalize_path(path: &Path) -> Result<PathBuf, TexoError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        use std::path::Component;
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    Ok(normalized)
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn replication_error(
    kind: ReplicationFailureKind,
    committed: Committed,
    detail: impl Into<String>,
) -> TexoError {
    TexoError::Replication {
        kind,
        committed,
        detail: detail.into(),
    }
}
