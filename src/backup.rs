//! Evidence-backed, offline-verifiable workspace backups.
//!
//! Only authority and the configuration needed to identify it are included:
//! the BatPak journal snapshot, `.texo/config.toml`, and `backup.json`.
//! Projection sidecars, model caches, generated views, and agent integration
//! files are deliberately excluded because they are rebuildable.
//! Unpinned verification detects corruption and incomplete publication, not a
//! coordinated rewrite of both data and manifest. Persist the create report's
//! `manifest_hash_hex` outside the backup and supply it to
//! [`verify_with_expected_manifest_hash`] when authenticity matters.

/// Backup manifest schema.
pub const MANIFEST_SCHEMA: &str = "texo.backup.v2";
pub(super) const MANIFEST_FILE: &str = "backup.json";
pub(super) const CONFIG_FILE: &str = "config.toml";
pub(super) const STORE_DIR: &str = "store";
pub(super) const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
pub(super) const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
pub(super) const MAX_STORE_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub(super) const MAX_STORE_FILES: usize = 100_000;

mod create;
mod filesystem;
mod model;
mod restore;
mod verify;

pub use create::create;
pub use model::{
    BackupCreateReport, BackupFinding, BackupManifest, BackupRestoreReport, BackupVerifyReport,
    FileRecord,
};
pub use restore::restore;
pub use verify::{verify, verify_with_expected_manifest_hash};
