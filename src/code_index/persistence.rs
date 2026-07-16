use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::TexoError;
use crate::events::ids::blake3_bytes_hex;
use crate::knowledge::{CodeIndexArtifact, CodeIndexId};

use super::util::source_error;
use super::{PreparedCodeIndex, ARTIFACT_SCHEMA};

static ARTIFACT_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Read a workspace-local SCIP file with a hard byte bound.
///
/// # Errors
/// Fails for paths outside the workspace, symlinks, non-regular files, and
/// files exceeding the declared bound.
pub fn read_scip(root: &Path, path: &Path, max_bytes: u64) -> Result<Vec<u8>, TexoError> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let canonical_root = std::fs::canonicalize(root)?;
    let canonical = std::fs::canonicalize(&candidate)?;
    if !canonical.starts_with(&canonical_root) {
        return Err(source_error(path, "SCIP path escapes the workspace"));
    }
    let metadata = std::fs::symlink_metadata(&candidate)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(source_error(
            path,
            "SCIP input must be a regular non-symlink file",
        ));
    }
    if metadata.len() > max_bytes {
        return Err(source_error(
            path,
            "SCIP input exceeds the configured byte limit",
        ));
    }
    let bytes = std::fs::read(&candidate)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != metadata.len() {
        return Err(source_error(path, "SCIP input changed while it was read"));
    }
    Ok(bytes)
}

/// Persist a normalized artifact with atomic content-addressed replacement.
///
/// # Errors
/// Returns an I/O error for staging, flush, rename, or directory sync failure.
pub fn persist(root: &Path, prepared: &PreparedCodeIndex) -> Result<PathBuf, TexoError> {
    let path = artifact_path(root, &prepared.artifact.index_id);
    atomic_write(&path, &prepared.bytes)?;
    Ok(path)
}

/// Load and authenticate one disposable normalized code index.
///
/// Missing artifacts return `Ok(None)` so callers can report degraded
/// coverage. Present but malformed or digest-mismatched artifacts fail closed.
///
/// # Errors
/// Returns a typed decode/source error when a present artifact is invalid.
pub fn load(
    root: &Path,
    index_id: &CodeIndexId,
    expected_digest_hex: &str,
) -> Result<Option<CodeIndexArtifact>, TexoError> {
    let path = artifact_path(root, index_id);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if blake3_bytes_hex(&bytes) != expected_digest_hex {
        return Err(source_error(&path, "code-index artifact digest mismatch"));
    }
    let artifact = batpak::encoding::from_bytes::<CodeIndexArtifact>(&bytes).map_err(|error| {
        TexoError::Decode {
            entity: index_id.to_string(),
            detail: error.to_string(),
        }
    })?;
    if artifact.schema != ARTIFACT_SCHEMA {
        return Ok(None);
    }
    if artifact.index_id != *index_id {
        return Err(source_error(&path, "code-index artifact identity mismatch"));
    }
    Ok(Some(artifact))
}

fn artifact_path(root: &Path, index_id: &CodeIndexId) -> PathBuf {
    root.join(".texo")
        .join("cache")
        .join("code-index")
        .join(format!("{}.bin", index_id.as_str()))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("artifact path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    for _attempt in 0..100 {
        let counter = ARTIFACT_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(
            ".texo-code-index-{}-{counter}.tmp",
            std::process::id()
        ));
        let mut file = match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, path)?;
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _removed = std::fs::remove_file(&tmp);
        }
        return result;
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a private code-index staging file",
    ))
}
