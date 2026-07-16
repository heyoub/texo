use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use batpak::event::EventPayload;
use batpak::store::{Open, ReadOnly, Store};

use crate::compat::batpak as substrate;
use crate::config::TexoRootConfig;
use crate::error::{Committed, ReplicationFailureKind, TexoError};
use crate::events::payloads::ReplicaBatchMaterializedV1;
use crate::topology::{JournalRole, ReplicaMode};

use super::evidence::{
    cursor_path, display_relative, ensure_fresh_destination, evidence_path, frontier, hex_lower,
    load_cursor, open_read_only_store, open_store, persist, replication_error, resolve_circuit,
    source_namespace, unchanged_report, validate_cursor_binding, validate_source_anchor,
};
use super::materialize::{replica_ledger_item, represented_source_events, verify_replica};
use super::remote::import_remote_from_cursor;
use super::{Circuit, ExactForkEvidence, ReplicaCursor, ReplicaReport, STATE_SCHEMA_VERSION};

const READER_REFRESH_ATTEMPTS: u32 = 50;
const READER_REFRESH_BACKOFF: Duration = Duration::from_millis(40);

#[derive(Default)]
struct LocalProgress {
    after: Option<u64>,
    imported: u64,
    deduplicated: u64,
    skipped_reserved: u64,
    skipped_operational: u64,
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
    if circuit.replica.source_endpoint.is_some() {
        import_remote_from_cursor(root, &circuit, Some(&cursor))
    } else {
        import_from_cursor(root, &circuit, Some(&cursor))
    }
}

/// Bring an imported reader journal to the latest source frontier before use.
///
/// Canonical journals and exact point-in-time forks are intentionally left
/// untouched. Imported replicas bootstrap when no cursor exists and otherwise
/// resume from their durable cursor. A short, bounded lease retry lets several
/// local agent clients start concurrently without weakening `BatPak`'s
/// single-owner contract for any physical store.
///
/// # Errors
/// Returns a typed replication failure when topology, evidence, transport, or
/// store ownership cannot be resolved within the bounded retry window.
pub fn refresh_reader(
    root: &Path,
    workspace_id: Option<&str>,
    journal_id: &str,
) -> Result<Option<ReplicaReport>, TexoError> {
    let config = TexoRootConfig::load(&root.join(".texo/config.toml")).map_err(|error| {
        replication_error(
            ReplicationFailureKind::InvalidTopology,
            Committed::No,
            error.to_string(),
        )
    })?;
    let (workspace, journal) = config
        .resolve_journal(workspace_id, Some(journal_id))
        .map_err(|error| {
            replication_error(
                ReplicationFailureKind::InvalidTopology,
                Committed::No,
                error.to_string(),
            )
        })?;
    if journal.role != JournalRole::Replica
        || journal.replica_mode != Some(ReplicaMode::ImportedReadModel)
    {
        return Ok(None);
    }
    for attempt in 1..=READER_REFRESH_ATTEMPTS {
        let result = if cursor_path(root, &workspace.workspace_id, journal_id).exists() {
            follow_once(root, Some(&workspace.workspace_id), journal_id)
        } else {
            bootstrap(root, Some(&workspace.workspace_id), journal_id)
        };
        match result {
            Ok(report) => return Ok(Some(report)),
            Err(TexoError::Replication {
                kind: ReplicationFailureKind::Busy,
                ..
            }) if attempt < READER_REFRESH_ATTEMPTS => {
                std::thread::sleep(READER_REFRESH_BACKOFF);
            }
            Err(error) => return Err(error),
        }
    }
    Err(replication_error(
        ReplicationFailureKind::Busy,
        Committed::No,
        "replica reader refresh exhausted its bounded lease retries",
    ))
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
    if circuit.replica.source_endpoint.is_some() {
        import_remote_from_cursor(root, circuit, None)
    } else {
        import_from_cursor(root, circuit, None)
    }
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
    let namespace = source_namespace(&circuit.workspace.workspace_id, circuit.source.id.as_str());
    let after = previous
        .as_ref()
        .and_then(|cursor| cursor.source_high_watermark);
    let source_ceiling = frontier(&source);
    if let Some(cursor) = unchanged_local_cursor(previous, source_ceiling) {
        return Ok(unchanged_report(root, circuit, cursor));
    }
    let destination = open_store(&circuit.replica_path, Committed::No)?;
    let represented = represented_source_events(
        &destination,
        &circuit.workspace.workspace_id,
        circuit.replica.id.as_str(),
        &namespace,
    )?;
    let progress = materialize_local_pages(
        &source,
        &destination,
        circuit,
        &namespace,
        &represented,
        source_ceiling,
        after,
    )?;
    finish_local_import(root, circuit, &source, &destination, namespace, &progress)
}

fn materialize_local_pages(
    source: &Store<ReadOnly>,
    destination: &Store<Open>,
    circuit: &Circuit,
    namespace: &str,
    represented: &BTreeSet<String>,
    source_ceiling: u64,
    after: Option<u64>,
) -> Result<LocalProgress, TexoError> {
    let mut progress = LocalProgress {
        after,
        ..LocalProgress::default()
    };
    loop {
        let page = substrate::read_import_page(
            source,
            progress.after,
            source_ceiling,
            represented,
            ReplicaBatchMaterializedV1::KIND,
        )
        .map_err(|error| {
            replication_error(
                ReplicationFailureKind::Substrate,
                Committed::Unknown,
                format!("source page read failed: {error}"),
            )
        })?;
        progress.deduplicated = progress.deduplicated.saturating_add(page.deduplicated);
        progress.skipped_reserved = progress
            .skipped_reserved
            .saturating_add(page.skipped_reserved);
        progress.skipped_operational = progress
            .skipped_operational
            .saturating_add(page.skipped_operational);
        let next = page.high_watermark;
        let has_more = page.has_more;
        if !page.events.is_empty() {
            let ledger = replica_ledger_item(circuit, namespace, &page.events)?;
            let count = substrate::append_with_ledger(destination, namespace, page.events, ledger)
                .map_err(|error| {
                    replication_error(
                        ReplicationFailureKind::Substrate,
                        Committed::Unknown,
                        format!("atomic replica batch failed: {error}"),
                    )
                })?;
            progress.imported = progress
                .imported
                .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
        }
        if next == progress.after || !has_more {
            progress.after = next;
            break;
        }
        progress.after = next;
    }
    Ok(progress)
}

fn finish_local_import(
    root: &Path,
    circuit: &Circuit,
    source: &Store<ReadOnly>,
    destination: &Store<Open>,
    namespace: String,
    progress: &LocalProgress,
) -> Result<ReplicaReport, TexoError> {
    verify_replica(destination)?;
    let source_high_watermark = progress.after;
    let source_anchor_event_id_hex =
        source_high_watermark.and_then(|sequence| substrate::event_id_at(source, sequence));
    if source_high_watermark.is_some() && source_anchor_event_id_hex.is_none() {
        return Err(replication_error(
            ReplicationFailureKind::AnchorMismatch,
            Committed::Partial,
            "source high watermark has no readable anchor event",
        ));
    }
    let replica_frontier = frontier(destination);
    let cursor = ReplicaCursor {
        schema_version: STATE_SCHEMA_VERSION,
        workspace_id: circuit.workspace.workspace_id.clone(),
        source_journal: circuit.source.id.to_string(),
        replica_journal: circuit.replica.id.to_string(),
        source_namespace: namespace,
        source_endpoint: None,
        source_high_watermark,
        source_anchor_event_id_hex,
        replica_frontier,
        replica_anchor_event_id_hex: substrate::event_id_at(destination, replica_frontier),
    };
    let path = cursor_path(
        root,
        &circuit.workspace.workspace_id,
        circuit.replica.id.as_str(),
    );
    persist(&path, &cursor, Committed::Partial)?;
    Ok(ReplicaReport::ImportedReadModel {
        imported: progress.imported,
        deduplicated: progress.deduplicated,
        skipped_reserved: progress.skipped_reserved,
        skipped_operational: progress.skipped_operational,
        cursor,
        cursor_path: display_relative(root, &path),
    })
}

fn unchanged_local_cursor(
    previous: Option<&ReplicaCursor>,
    source_ceiling: u64,
) -> Option<&ReplicaCursor> {
    previous.filter(|cursor| {
        cursor.source_high_watermark == Some(source_ceiling)
            || (source_ceiling == 0 && cursor.source_high_watermark.is_none())
    })
}
