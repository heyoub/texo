use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use batpak::store::{Open, ReadOnly, Store, StoreConfig, StoreState};
use serde::Serialize;

use crate::compat::batpak as substrate;
use crate::config::TexoRootConfig;
use crate::error::{Committed, ReplicationFailureKind, TexoError};
use crate::topology::JournalRole;

use super::{Circuit, ReplicaCursor, ReplicaReport, STATE_SCHEMA_VERSION};

pub(super) fn unchanged_report(
    root: &Path,
    circuit: &Circuit,
    cursor: &ReplicaCursor,
) -> ReplicaReport {
    let path = cursor_path(
        root,
        &circuit.workspace.workspace_id,
        circuit.replica.id.as_str(),
    );
    ReplicaReport::ImportedReadModel {
        imported: 0,
        deduplicated: 0,
        skipped_reserved: 0,
        skipped_operational: 0,
        cursor: cursor.clone(),
        cursor_path: display_relative(root, &path),
    }
}

pub(super) fn resolve_circuit(
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

pub(super) fn ensure_fresh_destination(path: &Path) -> Result<(), TexoError> {
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

pub(super) fn open_store(path: &Path, committed: Committed) -> Result<Store<Open>, TexoError> {
    Store::open(StoreConfig::new(path)).map_err(|error| {
        replication_error(
            store_error_kind(&error),
            committed,
            format!("store {}: {error}", path.display()),
        )
    })
}

pub(super) fn open_read_only_store(
    path: &Path,
    committed: Committed,
) -> Result<Store<ReadOnly>, TexoError> {
    Store::<ReadOnly>::open_read_only(StoreConfig::new(path)).map_err(|error| {
        replication_error(
            store_error_kind(&error),
            committed,
            format!("read-only store {}: {error}", path.display()),
        )
    })
}

pub(super) fn store_error_kind(error: &batpak::store::StoreError) -> ReplicationFailureKind {
    if matches!(error, batpak::store::StoreError::StoreLocked { .. }) {
        ReplicationFailureKind::Busy
    } else {
        ReplicationFailureKind::Substrate
    }
}

pub(super) fn validate_cursor_binding(
    cursor: &ReplicaCursor,
    circuit: &Circuit,
) -> Result<(), TexoError> {
    let expected_namespace =
        source_namespace(&circuit.workspace.workspace_id, circuit.source.id.as_str());
    if cursor.schema_version != STATE_SCHEMA_VERSION
        || cursor.workspace_id != circuit.workspace.workspace_id
        || cursor.source_journal != circuit.source.id.as_str()
        || cursor.replica_journal != circuit.replica.id.as_str()
        || cursor.source_namespace != expected_namespace
        || cursor.source_endpoint != circuit.replica.source_endpoint
    {
        return Err(replication_error(
            ReplicationFailureKind::Evidence,
            Committed::No,
            "replica cursor does not match the configured circuit",
        ));
    }
    Ok(())
}

pub(super) fn validate_source_anchor<S: StoreState>(
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

pub(super) fn load_cursor(
    root: &Path,
    workspace: &str,
    replica: &str,
) -> Result<ReplicaCursor, TexoError> {
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

pub(super) fn persist<T: Serialize>(
    path: &Path,
    value: &T,
    committed: Committed,
) -> Result<(), TexoError> {
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

pub(super) fn cursor_path(root: &Path, workspace: &str, replica: &str) -> PathBuf {
    evidence_path(root, workspace, replica, "cursor.msgpack")
}

pub(super) fn evidence_path(root: &Path, workspace: &str, replica: &str, file: &str) -> PathBuf {
    root.join(".texo")
        .join("replication")
        .join(workspace)
        .join(replica)
        .join(file)
}

pub(super) fn source_namespace(workspace: &str, source: &str) -> String {
    format!("texo.replica.v1:{workspace}:{source}")
}

pub(super) fn frontier<S: StoreState>(store: &Store<S>) -> u64 {
    store.frontier().visible_hlc.global_sequence
}

pub(super) fn normalize_path(path: &Path) -> Result<PathBuf, TexoError> {
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

pub(super) fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub(super) fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

pub(super) fn replication_error(
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
