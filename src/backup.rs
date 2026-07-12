//! Evidence-backed, offline-verifiable workspace backups.
//!
//! Only authority and the configuration needed to identify it are included:
//! the BatPak journal snapshot, `.texo/config.toml`, and `backup.json`.
//! Projection sidecars, model caches, generated views, and agent integration
//! files are deliberately excluded because they are rebuildable.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use batpak::store::{
    snapshot_report_body_hash, ReadOnly, SnapshotEvidenceReport, Store, StoreConfig,
    SNAPSHOT_EVIDENCE_REPORT_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};

use crate::config::WorkspaceConfig;
use crate::error::TexoError;

/// Backup manifest schema.
pub const MANIFEST_SCHEMA: &str = "texo.backup.v1";
const MANIFEST_FILE: &str = "backup.json";
const CONFIG_FILE: &str = "config.toml";
const STORE_DIR: &str = "store";
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_STORE_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_STORE_FILES: usize = 100_000;
static VERIFY_COPY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// One exact file recorded in backup evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRecord {
    /// File name relative to `store/`.
    pub name: String,
    /// Exact byte length.
    pub bytes: u64,
    /// BLAKE3 of the exact bytes.
    pub hash_hex: String,
}

/// Durable backup manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupManifest {
    /// Manifest schema.
    pub schema: String,
    /// Captured workspace.
    pub workspace_id: String,
    /// Store path from workspace config, for operator-led restore.
    pub store_path: String,
    /// Creation time supplied by the CLI.
    pub created_at_ms: u64,
    /// `BatPak` lifecycle evidence binding the snapshot.
    pub snapshot: SnapshotEvidenceReport,
    /// Exact journal snapshot file table.
    pub store_files: Vec<FileRecord>,
    /// Exact config size.
    pub config_bytes: u64,
    /// Exact config digest.
    pub config_hash_hex: String,
}

/// Successful backup creation report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupCreateReport {
    /// Report schema.
    pub schema: &'static str,
    /// Absolute immutable destination.
    pub dest: String,
    /// Captured workspace.
    pub workspace_id: String,
    /// Journal files captured.
    pub store_file_count: usize,
    /// Journal bytes captured.
    pub store_bytes: u64,
    /// Snapshot structural identity.
    pub snapshot_id_hex: String,
    /// Evidence manifest digest.
    pub manifest_hash_hex: String,
}

/// One stable verification finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupFinding {
    /// Stable finding class.
    pub kind: &'static str,
    /// Sanitized evidence.
    pub detail: String,
}

/// Offline backup verification report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupVerifyReport {
    /// Report schema.
    pub schema: &'static str,
    /// Whether every evidence check passed.
    pub verified: bool,
    /// Absolute destination inspected.
    pub dest: String,
    /// Workspace from a valid manifest, otherwise empty.
    pub workspace_id: String,
    /// Valid recorded store files.
    pub store_files_valid: usize,
    /// Expected recorded store files.
    pub store_files_expected: usize,
    /// Digest of manifest bytes found on disk.
    pub manifest_hash_hex: String,
    /// Content findings; empty on success.
    pub findings: Vec<BackupFinding>,
}

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
    let (config_hash_hex, config_bytes) = copy_config(root, destination.path())?;
    let manifest = BackupManifest {
        schema: MANIFEST_SCHEMA.to_string(),
        workspace_id: workspace.workspace_id.clone(),
        store_path: workspace.store_path.clone(),
        created_at_ms,
        snapshot: snapshot.clone(),
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
        manifest_hash_hex: blake3::hash(&manifest_bytes).to_hex().to_string(),
    })
}

/// Verify a backup using only bytes beneath its destination.
///
/// Content failures are findings, not function errors. Function errors are
/// reserved for environmental failures such as unreadable directory entries.
///
/// # Errors
/// Returns an error only when the destination cannot be safely inspected.
pub fn verify(dest: &Path) -> Result<BackupVerifyReport, TexoError> {
    let original = dest.to_path_buf();
    let metadata = match fs::symlink_metadata(dest) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(empty_report(
                &original,
                finding("backup_missing", "backup destination does not exist"),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_dir() {
        return Ok(empty_report(
            &original,
            finding(
                "backup_root_invalid",
                "backup root must be a regular directory",
            ),
        ));
    }
    let dest = absolute_path(dest)?;
    let manifest_path = dest.join(MANIFEST_FILE);
    let manifest_bytes = match read_regular_bounded(&manifest_path, MAX_MANIFEST_BYTES) {
        Ok(bytes) => bytes,
        Err(detail) => return Ok(empty_report(&dest, finding("manifest_invalid", detail))),
    };
    let manifest_hash_hex = blake3::hash(&manifest_bytes).to_hex().to_string();
    let manifest: BackupManifest = match serde_json::from_slice(&manifest_bytes) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Ok(empty_report_with_hash(
                &dest,
                manifest_hash_hex,
                finding("manifest_invalid", error.to_string()),
            ));
        }
    };
    let mut findings = Vec::new();
    if manifest.schema != MANIFEST_SCHEMA {
        findings.push(finding(
            "manifest_schema_unsupported",
            format!("unsupported schema `{}`", manifest.schema),
        ));
    }
    if manifest.store_files.len() > MAX_STORE_FILES {
        findings.push(finding(
            "manifest_file_limit",
            format!("manifest records more than {MAX_STORE_FILES} store files"),
        ));
    }
    check_top_level(&dest, &mut findings)?;
    check_snapshot_evidence(&dest, &manifest, &mut findings);
    let store_files_valid = check_store_files(&dest, &manifest, &mut findings)?;
    check_config(&dest, &manifest, &mut findings);
    if store_files_valid == manifest.store_files.len() {
        check_store_read_only(&dest, &manifest, &mut findings);
    }
    Ok(BackupVerifyReport {
        schema: "texo.backup-verify.v1",
        verified: findings.is_empty(),
        dest: dest.display().to_string(),
        workspace_id: manifest.workspace_id,
        store_files_valid,
        store_files_expected: manifest.store_files.len(),
        manifest_hash_hex,
        findings,
    })
}

fn check_snapshot_evidence(
    dest: &Path,
    manifest: &BackupManifest,
    findings: &mut Vec<BackupFinding>,
) {
    if manifest.snapshot.body.schema_version != SNAPSHOT_EVIDENCE_REPORT_SCHEMA_VERSION {
        findings.push(finding(
            "snapshot_schema_unsupported",
            format!(
                "unsupported snapshot schema {}",
                manifest.snapshot.body.schema_version
            ),
        ));
    }
    match snapshot_report_body_hash(&manifest.snapshot.body) {
        Ok(hash) if hash == manifest.snapshot.body_hash => {}
        Ok(_) => findings.push(finding(
            "snapshot_evidence_mismatch",
            "snapshot evidence body hash does not recompute",
        )),
        Err(error) => findings.push(finding(
            "snapshot_evidence_mismatch",
            format!("snapshot evidence cannot be encoded: {error}"),
        )),
    }
    let path_hash = blake3::hash(dest.join(STORE_DIR).as_os_str().as_encoded_bytes());
    if path_hash.as_bytes() != &manifest.snapshot.body.destination_path_digest {
        findings.push(finding(
            "snapshot_destination_mismatch",
            "snapshot evidence is bound to a different destination path",
        ));
    }
}

fn check_store_files(
    dest: &Path,
    manifest: &BackupManifest,
    findings: &mut Vec<BackupFinding>,
) -> Result<usize, TexoError> {
    let store_dir = dest.join(STORE_DIR);
    match fs::symlink_metadata(&store_dir) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            findings.push(finding(
                "store_directory_invalid",
                "store must be a regular directory, not a symbolic link or file",
            ));
            return Ok(0);
        }
        Err(error) => {
            findings.push(finding("store_directory_invalid", error.to_string()));
            return Ok(0);
        }
    }
    let mut expected = std::collections::BTreeSet::new();
    let mut valid = 0;
    for record in &manifest.store_files {
        if !safe_flat_name(&record.name) || !expected.insert(record.name.clone()) {
            findings.push(finding(
                "store_record_invalid",
                format!("unsafe or duplicate store file `{}`", record.name),
            ));
            continue;
        }
        let path = store_dir.join(&record.name);
        match hash_regular_bounded(&path, MAX_STORE_FILE_BYTES) {
            Ok((hash, bytes)) if hash == record.hash_hex && bytes == record.bytes => valid += 1,
            Ok((hash, bytes)) => findings.push(finding(
                "store_file_mismatch",
                format!(
                    "{} expected {} bytes/{} but found {bytes}/{hash}",
                    record.name, record.bytes, record.hash_hex
                ),
            )),
            Err(detail) => findings.push(finding("store_file_invalid", detail)),
        }
    }
    match fs::read_dir(&store_dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().to_string();
                if !expected.contains(&name) {
                    findings.push(finding(
                        "unexpected_store_file",
                        format!("unrecorded store entry `{name}`"),
                    ));
                }
            }
        }
        Err(error) => findings.push(finding("store_directory_invalid", error.to_string())),
    }
    Ok(valid)
}

fn check_config(dest: &Path, manifest: &BackupManifest, findings: &mut Vec<BackupFinding>) {
    match hash_regular_bounded(&dest.join(CONFIG_FILE), MAX_CONFIG_BYTES) {
        Ok((hash, bytes)) if hash == manifest.config_hash_hex && bytes == manifest.config_bytes => {
        }
        Ok((hash, bytes)) => findings.push(finding(
            "config_mismatch",
            format!(
                "expected {}/{} but found {bytes}/{hash}",
                manifest.config_bytes, manifest.config_hash_hex
            ),
        )),
        Err(detail) => findings.push(finding("config_invalid", detail)),
    }
}

fn check_store_read_only(
    dest: &Path,
    manifest: &BackupManifest,
    findings: &mut Vec<BackupFinding>,
) {
    let copy = match VerificationCopy::create(&dest.join(STORE_DIR), &manifest.store_files) {
        Ok(copy) => copy,
        Err(error) => {
            findings.push(finding(
                "store_open_invalid",
                format!("snapshot cannot be prepared for read-only verification: {error}"),
            ));
            return;
        }
    };
    match Store::<ReadOnly>::open_read_only(StoreConfig::new(copy.path())) {
        Ok(store) => match store.verify_chain() {
            Ok(chain) if chain.is_intact() => {}
            Ok(chain) => findings.push(finding(
                "store_chain_invalid",
                format!("snapshot chain verification failed: {chain:?}"),
            )),
            Err(error) => findings.push(finding(
                "store_chain_invalid",
                format!("snapshot chain verification errored: {error}"),
            )),
        },
        Err(error) => findings.push(finding(
            "store_open_invalid",
            format!("snapshot cannot open read-only: {error}"),
        )),
    }
}

struct VerificationCopy {
    path: PathBuf,
}

impl VerificationCopy {
    fn create(source: &Path, records: &[FileRecord]) -> Result<Self, TexoError> {
        let parent = std::env::temp_dir();
        for _attempt in 0..100 {
            let counter = VERIFY_COPY_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                "texo-backup-verify-{}-{counter}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    let copy = Self { path };
                    for record in records {
                        if !safe_flat_name(&record.name) {
                            return Err(backup_error("unsafe store record in verification copy"));
                        }
                        fs::copy(source.join(&record.name), copy.path.join(&record.name))?;
                    }
                    return Ok(copy);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(backup_error("could not allocate verification directory"))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for VerificationCopy {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.path);
    }
}

fn check_top_level(dest: &Path, findings: &mut Vec<BackupFinding>) -> Result<(), TexoError> {
    let expected = std::collections::BTreeSet::from([MANIFEST_FILE, CONFIG_FILE, STORE_DIR]);
    for entry in fs::read_dir(dest)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !expected.contains(name.as_str()) {
            findings.push(finding(
                "unexpected_backup_entry",
                format!("unrecorded top-level entry `{name}`"),
            ));
        }
    }
    Ok(())
}

fn hash_store_files(directory: &Path) -> Result<Vec<FileRecord>, TexoError> {
    let mut records = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.file_type().is_file() {
            return Err(backup_error(format!(
                "snapshot entry `{}` is not a regular file",
                entry.path().display()
            )));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| backup_error("snapshot file name is not UTF-8"))?;
        if records.len() >= MAX_STORE_FILES {
            return Err(backup_error(format!(
                "snapshot exceeds the {MAX_STORE_FILES}-file limit"
            )));
        }
        let (hash_hex, bytes) =
            hash_regular_bounded(&entry.path(), MAX_STORE_FILE_BYTES).map_err(backup_error)?;
        records.push(FileRecord {
            name,
            bytes,
            hash_hex,
        });
    }
    records.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(records)
}

fn copy_config(root: &Path, dest: &Path) -> Result<(String, u64), TexoError> {
    let source = root.join(".texo/config.toml");
    let bytes = read_regular_bounded(&source, MAX_CONFIG_BYTES).map_err(backup_error)?;
    write_new_synced(&dest.join(CONFIG_FILE), &bytes)?;
    Ok((
        blake3::hash(&bytes).to_hex().to_string(),
        bytes.len() as u64,
    ))
}

fn read_regular_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > limit {
        return Err(format!("{} exceeds the {limit}-byte limit", path.display()));
    }
    fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn hash_regular_bounded(path: &Path, limit: u64) -> Result<(String, u64), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.len() > limit {
        return Err(format!("{} is not a bounded regular file", path.display()));
    }
    let mut file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        bytes = bytes.saturating_add(count as u64);
        if bytes > limit {
            return Err(format!(
                "{} grew beyond the {limit}-byte limit",
                path.display()
            ));
        }
        hasher.update(&buffer[..count]);
    }
    Ok((hasher.finalize().to_hex().to_string(), bytes))
}

fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<(), TexoError> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn reject_overlap(root: &Path, workspace: &WorkspaceConfig, dest: &Path) -> Result<(), TexoError> {
    let root = absolute_path(root)?;
    let texo = absolute_path(&root.join(".texo"))?;
    let store = absolute_path(&workspace.store_path_buf(&root))?;
    for live in [&root, &texo, &store] {
        if dest.starts_with(live) || live.starts_with(dest) {
            return Err(backup_error(
                "backup destination must not overlap the workspace root, .texo, or live store",
            ));
        }
    }
    Ok(())
}

fn safe_flat_name(name: &str) -> bool {
    !name.is_empty()
        && Path::new(name)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && Path::new(name).file_name().and_then(|value| value.to_str()) == Some(name)
}

fn absolute_path(path: &Path) -> Result<PathBuf, TexoError> {
    let absolute = std::path::absolute(path)?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(backup_error("path escapes the filesystem root"));
                }
            }
            kept @ (Component::Prefix(_) | Component::RootDir | Component::Normal(_)) => {
                normalized.push(kept);
            }
        }
    }
    let mut existing = normalized.clone();
    let mut missing = Vec::<OsString>::new();
    loop {
        match fs::symlink_metadata(&existing) {
            Ok(_) => {
                let mut resolved = fs::canonicalize(&existing)?;
                for name in missing.iter().rev() {
                    resolved.push(name);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = existing
                    .file_name()
                    .ok_or_else(|| backup_error("path has no existing ancestor"))?;
                missing.push(name.to_os_string());
                if !existing.pop() {
                    return Err(backup_error("path has no existing ancestor"));
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn sync_directory(path: &Path) -> Result<(), TexoError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn finding(kind: &'static str, detail: impl Into<String>) -> BackupFinding {
    BackupFinding {
        kind,
        detail: detail.into(),
    }
}

fn backup_error(detail: impl Into<String>) -> TexoError {
    TexoError::Backup {
        detail: detail.into(),
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(
        String::with_capacity(bytes.len().saturating_mul(2)),
        |mut encoded, byte| {
            let _result = write!(encoded, "{byte:02x}");
            encoded
        },
    )
}

fn empty_report(dest: &Path, finding: BackupFinding) -> BackupVerifyReport {
    empty_report_with_hash(dest, String::new(), finding)
}

fn empty_report_with_hash(
    dest: &Path,
    manifest_hash_hex: String,
    finding: BackupFinding,
) -> BackupVerifyReport {
    BackupVerifyReport {
        schema: "texo.backup-verify.v1",
        verified: false,
        dest: dest.display().to_string(),
        workspace_id: String::new(),
        store_files_valid: 0,
        store_files_expected: 0,
        manifest_hash_hex,
        findings: vec![finding],
    }
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
