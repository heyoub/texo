use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use batpak::store::{ReadOnly, Store, StoreConfig};

use crate::error::TexoError;

use super::filesystem::{
    absolute_path, backup_error, hash_regular_bounded, read_regular_bounded, safe_flat_name,
    sync_directory, write_new_synced,
};
use super::verify::verify_with_expected_manifest_hash;
use super::{
    BackupManifest, BackupRestoreReport, FileRecord, CONFIG_FILE, MANIFEST_FILE,
    MAX_MANIFEST_BYTES, MAX_STORE_FILE_BYTES, STORE_DIR,
};

static RESTORE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Restore a verified backup into a fresh workspace root.
///
/// The destination must not exist. Restore verifies the source first, copies
/// only manifest-listed regular files into a private sibling staging root,
/// rewrites the selected workspace store path to a safe root-relative default,
/// verifies the copied `BatPak` chain, and atomically renames the staging root
/// into place. Derived caches and client integration are intentionally absent.
///
/// # Errors
/// Returns an error for an invalid backup, overlapping/existing destination,
/// unsafe source bytes, copied-file mismatch, or failed restored chain.
pub fn restore(
    backup: &Path,
    dest: &Path,
    expected_manifest_hash: Option<&str>,
) -> Result<BackupRestoreReport, TexoError> {
    let verification = verify_with_expected_manifest_hash(backup, expected_manifest_hash)?;
    if !verification.verified {
        return Err(backup_error(format!(
            "backup restore refused: {}",
            verification
                .findings
                .iter()
                .map(|finding| finding.kind)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let backup = absolute_path(backup)?;
    let dest = absolute_path(dest)?;
    if dest.starts_with(&backup) || backup.starts_with(&dest) {
        return Err(backup_error(
            "restore destination must not overlap the backup",
        ));
    }
    let manifest_bytes = read_regular_bounded(&backup.join(MANIFEST_FILE), MAX_MANIFEST_BYTES)
        .map_err(backup_error)?;
    let manifest: BackupManifest = serde_json::from_slice(&manifest_bytes)?;
    let source_config = crate::config::TexoRootConfig::load(&backup.join(CONFIG_FILE))
        .map_err(|error| backup_error(error.to_string()))?;
    let mut workspace = source_config
        .workspaces
        .get(&manifest.workspace_id)
        .cloned()
        .ok_or_else(|| backup_error("backup config does not contain its workspace"))?;
    let restore_store_path = crate::config::WorkspaceEntry::for_id(&manifest.workspace_id)
        .primary()
        .map_err(|error| backup_error(error.to_string()))?
        .store_path;
    workspace
        .set_primary_store_path(restore_store_path.clone())
        .map_err(|error| backup_error(error.to_string()))?;
    let mut workspaces = std::collections::BTreeMap::new();
    workspaces.insert(manifest.workspace_id.clone(), workspace.clone());
    let restored_config = crate::config::TexoRootConfig {
        default_workspace: manifest.workspace_id.clone(),
        workspaces,
        gateway: source_config.gateway,
    };
    let destination = RestoreDestination::create(&dest)?;
    let texo_dir = destination.path().join(".texo");
    fs::create_dir(&texo_dir)?;
    let config_bytes = toml::to_string_pretty(&restored_config)
        .map_err(|error| backup_error(error.to_string()))?;
    write_new_synced(&texo_dir.join(CONFIG_FILE), config_bytes.as_bytes())?;
    let store_dest = destination.path().join(&restore_store_path);
    fs::create_dir_all(&store_dest)?;
    copy_verified_store(&backup.join(STORE_DIR), &store_dest, &manifest.store_files)?;
    let store = Store::<ReadOnly>::open_read_only(StoreConfig::new(&store_dest))?;
    let chain = store.verify_chain()?;
    if !chain.is_intact() {
        return Err(backup_error(format!(
            "restored store chain verification failed: {chain:?}"
        )));
    }
    drop(store);
    sync_directory(&store_dest)?;
    sync_directory(&texo_dir)?;
    destination.complete()?;
    Ok(BackupRestoreReport {
        schema: "texo.backup-restore.v1",
        dest: dest.display().to_string(),
        workspace_id: manifest.workspace_id,
        store_file_count: manifest.store_files.len(),
        store_bytes: manifest.store_files.iter().map(|record| record.bytes).sum(),
        manifest_hash_hex: verification.manifest_hash_hex,
        chain_verified: true,
    })
}

fn copy_verified_store(
    source: &Path,
    dest: &Path,
    records: &[FileRecord],
) -> Result<(), TexoError> {
    for record in records {
        if !safe_flat_name(&record.name) {
            return Err(backup_error("unsafe store record during restore"));
        }
        let target = dest.join(&record.name);
        fs::copy(source.join(&record.name), &target)?;
        File::open(&target)?.sync_all()?;
        let (hash, bytes) =
            hash_regular_bounded(&target, MAX_STORE_FILE_BYTES).map_err(backup_error)?;
        if hash != record.hash_hex || bytes != record.bytes {
            return Err(backup_error(format!(
                "restored copy of {} differs from verified source",
                record.name
            )));
        }
    }
    Ok(())
}

struct RestoreDestination {
    final_path: PathBuf,
    stage_path: PathBuf,
    complete: bool,
}

impl RestoreDestination {
    fn create(dest: &Path) -> Result<Self, TexoError> {
        match fs::symlink_metadata(dest) {
            Ok(_) => return Err(backup_error("restore destination already exists")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let parent = dest
            .parent()
            .ok_or_else(|| backup_error("restore destination has no parent"))?;
        fs::create_dir_all(parent)?;
        for _attempt in 0..100 {
            let sequence = RESTORE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let stage_path =
                parent.join(format!(".texo-restore-{}-{sequence}", std::process::id()));
            let created = {
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt as _;
                    builder.mode(0o700);
                }
                builder.create(&stage_path)
            };
            match created {
                Ok(()) => {
                    return Ok(Self {
                        final_path: dest.to_path_buf(),
                        stage_path,
                        complete: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(backup_error(
            "could not allocate a private restore staging root",
        ))
    }

    fn path(&self) -> &Path {
        &self.stage_path
    }

    fn complete(mut self) -> Result<(), TexoError> {
        sync_directory(&self.stage_path)?;
        fs::rename(&self.stage_path, &self.final_path)?;
        if let Some(parent) = self.final_path.parent() {
            sync_directory(parent)?;
        }
        self.complete = true;
        Ok(())
    }
}

impl Drop for RestoreDestination {
    fn drop(&mut self) {
        if !self.complete {
            let _ignored = fs::remove_dir_all(&self.stage_path);
        }
    }
}
