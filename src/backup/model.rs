//! Durable backup evidence and report shapes.

use batpak::store::backup_envelope::BackupManifestBody;
use batpak::store::SnapshotEvidenceReport;
use serde::{Deserialize, Serialize};

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
    /// BatPak-native canonical identity for the authority-bearing segment set.
    pub substrate_manifest: BackupManifestBody,
    /// Canonical digest of `substrate_manifest`, pinned inside the product envelope.
    pub substrate_manifest_hash_hex: String,
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
    /// BatPak-native canonical segment-manifest digest.
    pub substrate_manifest_hash_hex: String,
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

/// Successful restore into a fresh workspace root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupRestoreReport {
    /// Report schema.
    pub schema: &'static str,
    /// Fresh workspace root published atomically.
    pub dest: String,
    /// Restored workspace id.
    pub workspace_id: String,
    /// Journal files restored.
    pub store_file_count: usize,
    /// Journal bytes restored.
    pub store_bytes: u64,
    /// Verified source manifest digest.
    pub manifest_hash_hex: String,
    /// Whether the restored `BatPak` chain verified before publication.
    pub chain_verified: bool,
}
