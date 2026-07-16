use std::fs;
use std::path::{Path, PathBuf};

use batpak::store::backup_envelope::{
    backup_manifest_body_hash, BackupManifestBody, BackupSegmentRef,
    BACKUP_MANIFEST_BODY_SCHEMA_VERSION,
};
use batpak::store::{SnapshotEvidenceReport, Store};

use crate::config::WorkspaceConfig;
use crate::error::TexoError;

use super::filesystem::{
    absolute_path, backup_error, copy_config, digest_from_hex, hash_store_files, hex_bytes,
    reject_overlap, sync_directory, write_new_synced,
};
use super::verify::verify;
use super::{
    BackupCreateReport, BackupManifest, FileRecord, MANIFEST_FILE, MANIFEST_SCHEMA, STORE_DIR,
};

/// Create a fresh evidence-backed backup.
///
/// The destination directory is allocated exclusively and the manifest is
/// published last. `BatPak` evidence binds the final `store/` path, so staging
/// the snapshot under a renamed parent would invalidate that evidence. A crash
/// before manifest publication therefore leaves an honestly incomplete,
/// unverifiable directory rather than a falsely valid backup.
///
/// # Errors
/// Returns an error for an existing/overlapping destination, unsafe source
/// files, snapshot failure, or a backup that fails its own verification.
pub fn create(
    root: &Path,
    workspace: &WorkspaceConfig,
    store: &Store<batpak::store::Open>,
    dest: &Path,
    created_at_ms: u64,
) -> Result<BackupCreateReport, TexoError> {
    let dest = absolute_path(dest)?;
    reject_overlap(root, workspace, &dest)?;
    let destination = BackupDestination::create(&dest)?;
    let store_dest = destination.path().join(STORE_DIR);
    let snapshot = store.snapshot_with_evidence(&store_dest)?;
    let store_files = hash_store_files(&store_dest)?;
    let substrate_manifest = build_substrate_manifest(&snapshot, &store_files)?;
    let substrate_manifest_hash_hex = hex_bytes(
        &backup_manifest_body_hash(&substrate_manifest).map_err(|error| {
            backup_error(format!("substrate manifest encoding failed: {error}"))
        })?,
    );
    let (config_hash_hex, config_bytes) = copy_config(root, destination.path())?;
    let manifest = BackupManifest {
        schema: MANIFEST_SCHEMA.to_string(),
        workspace_id: workspace.workspace_id.clone(),
        store_path: workspace.store_path.clone(),
        created_at_ms,
        snapshot: snapshot.clone(),
        substrate_manifest,
        substrate_manifest_hash_hex: substrate_manifest_hash_hex.clone(),
        store_files: store_files.clone(),
        config_bytes,
        config_hash_hex,
    };
    let mut manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    manifest_bytes.push(b'\n');
    write_new_synced(&destination.path().join(MANIFEST_FILE), &manifest_bytes)?;
    sync_directory(destination.path())?;

    let verified = verify(destination.path())?;
    if !verified.verified {
        return Err(backup_error(format!(
            "prepared backup failed self-verification: {}",
            verified
                .findings
                .iter()
                .map(|finding| finding.kind)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    destination.complete()?;
    Ok(BackupCreateReport {
        schema: "texo.backup-create.v1",
        dest: dest.display().to_string(),
        workspace_id: workspace.workspace_id.clone(),
        store_file_count: store_files.len(),
        store_bytes: store_files.iter().map(|record| record.bytes).sum(),
        snapshot_id_hex: hex_bytes(&snapshot.body.snapshot_id),
        substrate_manifest_hash_hex,
        manifest_hash_hex: blake3::hash(&manifest_bytes).to_hex().to_string(),
    })
}

fn build_substrate_manifest(
    snapshot: &SnapshotEvidenceReport,
    records: &[FileRecord],
) -> Result<BackupManifestBody, TexoError> {
    let segments = segment_refs_from_records(records)?;
    let expected_ids = snapshot
        .body
        .copied_segment_ids_sorted
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let observed_ids = segments
        .iter()
        .map(|segment| segment.segment_id)
        .collect::<std::collections::BTreeSet<_>>();
    if expected_ids != observed_ids {
        return Err(backup_error(
            "snapshot segment evidence does not match copied segment files",
        ));
    }
    Ok(BackupManifestBody {
        schema_version: BACKUP_MANIFEST_BODY_SCHEMA_VERSION,
        backup_id: snapshot.body.snapshot_id,
        layout_revision: 1,
        tooling_revision: 1,
        segments,
    })
}

fn segment_refs_from_records(records: &[FileRecord]) -> Result<Vec<BackupSegmentRef>, TexoError> {
    let mut segments = Vec::new();
    for record in records {
        if Path::new(&record.name).extension() != Some(std::ffi::OsStr::new("fbat")) {
            continue;
        }
        let stem = record
            .name
            .strip_suffix(".fbat")
            .ok_or_else(|| backup_error("segment filename has no numeric stem"))?;
        let segment_id = stem
            .parse::<u64>()
            .map_err(|_| backup_error(format!("invalid segment filename `{}`", record.name)))?;
        if format!("{segment_id:06}.fbat") != record.name {
            return Err(backup_error(format!(
                "non-canonical segment filename `{}`",
                record.name
            )));
        }
        let bytes_digest = digest_from_hex(&record.hash_hex)
            .ok_or_else(|| backup_error(format!("invalid segment digest for `{}`", record.name)))?;
        segments.push(BackupSegmentRef {
            segment_id,
            bytes_digest,
        });
    }
    segments.sort();
    Ok(segments)
}

struct BackupDestination {
    path: PathBuf,
    complete: bool,
}

impl BackupDestination {
    fn create(dest: &Path) -> Result<Self, TexoError> {
        match fs::symlink_metadata(dest) {
            Ok(_) => return Err(backup_error("backup destination already exists")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let parent = dest
            .parent()
            .ok_or_else(|| backup_error("backup destination has no parent"))?;
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700).create(dest)?;
        }
        #[cfg(not(unix))]
        fs::create_dir(dest)?;
        Ok(Self {
            path: dest.to_path_buf(),
            complete: false,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn complete(mut self) -> Result<(), TexoError> {
        if let Some(parent) = self.path().parent() {
            sync_directory(parent)?;
        }
        self.complete = true;
        Ok(())
    }
}

impl Drop for BackupDestination {
    fn drop(&mut self) {
        if !self.complete {
            let _ignored = fs::remove_dir_all(&self.path);
        }
    }
}
