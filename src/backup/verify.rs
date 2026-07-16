use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use batpak::store::backup_envelope::{
    backup_manifest_body_hash, restore_proof_evidence_report, BackupSegmentRef,
    BACKUP_MANIFEST_BODY_SCHEMA_VERSION,
};
use batpak::store::{
    snapshot_report_body_hash, ReadOnly, Store, StoreConfig,
    SNAPSHOT_EVIDENCE_REPORT_SCHEMA_VERSION,
};

use crate::error::TexoError;

use super::filesystem::{
    absolute_path, backup_error, digest_from_hex, finding, hash_regular_bounded,
    read_regular_bounded, safe_flat_name,
};
use super::{
    BackupFinding, BackupManifest, BackupVerifyReport, FileRecord, CONFIG_FILE, MANIFEST_FILE,
    MANIFEST_SCHEMA, MAX_CONFIG_BYTES, MAX_MANIFEST_BYTES, MAX_STORE_FILES, MAX_STORE_FILE_BYTES,
    STORE_DIR,
};

static VERIFY_COPY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Verify a backup using only bytes beneath its destination.
///
/// Content failures are findings, not function errors. Function errors are
/// reserved for environmental failures such as unreadable directory entries.
///
/// # Errors
/// Returns an error only when the destination cannot be safely inspected.
pub fn verify(dest: &Path) -> Result<BackupVerifyReport, TexoError> {
    verify_with_expected_manifest_hash(dest, None)
}

/// Verify a backup and optionally compare its manifest to an out-of-band pin.
///
/// # Errors
/// Returns an input error for a malformed expected digest, or an environment
/// error when the destination cannot be safely inspected.
pub fn verify_with_expected_manifest_hash(
    dest: &Path,
    expected_manifest_hash: Option<&str>,
) -> Result<BackupVerifyReport, TexoError> {
    if let Some(expected) = expected_manifest_hash {
        if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(TexoError::OpInput {
                op: "texo backup verify".to_string(),
                detail: "expected manifest hash must be exactly 64 hexadecimal characters"
                    .to_string(),
            });
        }
    }
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
    if expected_manifest_hash.is_some_and(|expected| expected != manifest_hash_hex) {
        findings.push(finding(
            "manifest_hash_mismatch",
            format!("manifest hash is {manifest_hash_hex}; it does not match the out-of-band pin"),
        ));
    }
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
    check_substrate_restore_proof(&dest, &manifest, &mut findings);
    let store_files_valid = check_store_files(&dest, &manifest, &mut findings)?;
    let config_valid = check_config(&dest, &manifest, &mut findings);
    if config_valid {
        check_config_binding(&dest, &manifest, &mut findings);
    }
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

fn check_substrate_restore_proof(
    dest: &Path,
    manifest: &BackupManifest,
    findings: &mut Vec<BackupFinding>,
) {
    if manifest.substrate_manifest.schema_version != BACKUP_MANIFEST_BODY_SCHEMA_VERSION {
        findings.push(finding(
            "substrate_manifest_schema_unsupported",
            format!(
                "unsupported substrate manifest schema {}",
                manifest.substrate_manifest.schema_version
            ),
        ));
    }
    if manifest.substrate_manifest.backup_id != manifest.snapshot.body.snapshot_id {
        findings.push(finding(
            "substrate_manifest_snapshot_mismatch",
            "substrate backup identity does not match snapshot identity",
        ));
    }
    let claimed_hash = digest_from_hex(&manifest.substrate_manifest_hash_hex);
    match backup_manifest_body_hash(&manifest.substrate_manifest) {
        Ok(computed) if claimed_hash == Some(computed) => {}
        Ok(_) => findings.push(finding(
            "substrate_manifest_hash_mismatch",
            "substrate manifest body hash does not recompute",
        )),
        Err(error) => findings.push(finding(
            "substrate_manifest_invalid",
            format!("substrate manifest cannot be encoded: {error}"),
        )),
    }
    let mut observed = Vec::new();
    let store_dir = dest.join(STORE_DIR);
    let entries = match fs::read_dir(&store_dir) {
        Ok(entries) => entries,
        Err(error) => {
            findings.push(finding(
                "substrate_restore_proof_invalid",
                error.to_string(),
            ));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                findings.push(finding(
                    "substrate_restore_proof_invalid",
                    error.to_string(),
                ));
                return;
            }
        };
        let name = entry.file_name().to_string_lossy().to_string();
        let path = Path::new(&name);
        if path.extension() != Some(std::ffi::OsStr::new("fbat")) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(std::ffi::OsStr::to_str) else {
            continue;
        };
        let segment_id = match stem.parse::<u64>() {
            Ok(segment_id) if format!("{segment_id:06}.fbat") == name => segment_id,
            _ => {
                findings.push(finding(
                    "substrate_restore_proof_invalid",
                    format!("invalid segment filename `{name}`"),
                ));
                continue;
            }
        };
        match hash_regular_bounded(&entry.path(), MAX_STORE_FILE_BYTES) {
            Ok((hash, _bytes)) => match digest_from_hex(&hash) {
                Some(bytes_digest) => observed.push(BackupSegmentRef {
                    segment_id,
                    bytes_digest,
                }),
                None => findings.push(finding(
                    "substrate_restore_proof_invalid",
                    format!("invalid digest for `{name}`"),
                )),
            },
            Err(detail) => findings.push(finding("substrate_restore_proof_invalid", detail)),
        }
    }
    match restore_proof_evidence_report(&manifest.substrate_manifest, &observed) {
        Ok(report) if report.body.findings.is_empty() => {}
        Ok(report) => findings.push(finding(
            "substrate_restore_proof_failed",
            format!("BatPak restore proof findings: {:?}", report.body.findings),
        )),
        Err(error) => findings.push(finding(
            "substrate_restore_proof_invalid",
            format!("BatPak restore proof cannot be encoded: {error}"),
        )),
    }
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

fn check_config(dest: &Path, manifest: &BackupManifest, findings: &mut Vec<BackupFinding>) -> bool {
    match hash_regular_bounded(&dest.join(CONFIG_FILE), MAX_CONFIG_BYTES) {
        Ok((hash, bytes)) if hash == manifest.config_hash_hex && bytes == manifest.config_bytes => {
            true
        }
        Ok((hash, bytes)) => {
            findings.push(finding(
                "config_mismatch",
                format!(
                    "expected {}/{} but found {bytes}/{hash}",
                    manifest.config_bytes, manifest.config_hash_hex
                ),
            ));
            false
        }
        Err(detail) => {
            findings.push(finding("config_invalid", detail));
            false
        }
    }
}

fn check_config_binding(dest: &Path, manifest: &BackupManifest, findings: &mut Vec<BackupFinding>) {
    let config = match crate::config::TexoRootConfig::load(&dest.join(CONFIG_FILE)) {
        Ok(config) => config,
        Err(error) => {
            findings.push(finding("config_binding_invalid", error.to_string()));
            return;
        }
    };
    match config.resolve(Some(&manifest.workspace_id)) {
        Ok(workspace) if workspace.store_path == manifest.store_path => {}
        Ok(workspace) => findings.push(finding(
            "config_binding_mismatch",
            format!(
                "manifest store path `{}` differs from config `{}`",
                manifest.store_path, workspace.store_path
            ),
        )),
        Err(error) => findings.push(finding("config_binding_mismatch", error.to_string())),
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
