//! Deletable process boundary for the bvisor 0.10 extractor helper.
//!
//! bvisor's current event kinds collide with downstream domain payloads
//! (freebatteryfactory/batpak#233), so linking it into the Texo binary would
//! correctly trip BatPak's global registry gate. The dedicated helper keeps the
//! bvisor inventory in a separate process. The helper boundary disappears when
//! the family allocation and self-hosted launcher contracts ship.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_EXTRACTOR_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;
static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Stable extractor-boundary failures.
#[derive(Debug, thiserror::Error)]
pub enum ExtractorBoundaryError {
    /// The isolated bvisor helper was not installed safely.
    #[error("bvisor helper unavailable: {0}")]
    Helper(String),
    /// The host could not stage or retrieve the bounded workload artifact.
    #[error("bvisor artifact boundary: {0}")]
    Artifact(String),
    /// The helper or confined workload did not complete successfully.
    #[error("bvisor extractor did not complete successfully: {0}")]
    Workload(String),
}

/// Execute one configured extractor through the isolated bvisor helper.
///
/// # Errors
/// Returns a typed helper, workload, or artifact failure. There is no raw
/// command fallback.
pub fn run_extractor(cmd: &str, input_path: &Path) -> Result<Vec<u8>, ExtractorBoundaryError> {
    let helper = helper_path()?;
    let run = PrivateRun::create()?;
    let staged_input = run.path.join("input.md");
    fs::copy(input_path, &staged_input)
        .map_err(|error| ExtractorBoundaryError::Artifact(error.to_string()))?;
    File::open(&staged_input)
        .and_then(|file| file.sync_all())
        .map_err(|error| ExtractorBoundaryError::Artifact(error.to_string()))?;
    let output = run.path.join("claims.ndjson");
    let status = Command::new(helper)
        .arg("--cmd")
        .arg(cmd)
        .arg("--input")
        .arg(&staged_input)
        .arg("--output")
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| ExtractorBoundaryError::Helper(error.to_string()))?;
    if !status.success() {
        return Err(ExtractorBoundaryError::Workload(format!(
            "helper exited with {status}"
        )));
    }
    read_regular_bounded(&output)
}

/// Check whether both temporary bvisor companion processes are available.
///
/// # Errors
/// Returns a sanitized setup failure suitable for doctor output.
pub fn readiness() -> Result<String, String> {
    let helper = helper_path().map_err(|error| error.to_string())?;
    let launcher = std::env::var_os("BVISOR_LAUNCHER_BIN")
        .map(PathBuf::from)
        .ok_or_else(|| "BVISOR_LAUNCHER_BIN is not set".to_string())?;
    if !launcher.is_absolute() {
        return Err("BVISOR_LAUNCHER_BIN must be absolute".to_string());
    }
    let metadata = fs::symlink_metadata(&launcher).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("bvisor launcher must be a regular file, not a symlink".to_string());
    }
    Ok(format!(
        "isolated helper {} with launcher {}",
        helper.display(),
        launcher.display()
    ))
}

fn helper_path() -> Result<PathBuf, ExtractorBoundaryError> {
    let path = if let Some(value) = std::env::var_os("TEXO_BVISOR_HELPER_BIN") {
        PathBuf::from(value)
    } else {
        let current = std::env::current_exe()
            .map_err(|error| ExtractorBoundaryError::Helper(error.to_string()))?;
        current
            .parent()
            .ok_or_else(|| {
                ExtractorBoundaryError::Helper(
                    "Texo executable has no parent directory".to_string(),
                )
            })?
            .join("texo-bvisor-extractor")
    };
    if !path.is_absolute() {
        return Err(ExtractorBoundaryError::Helper(
            "helper path must be absolute".to_string(),
        ));
    }
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| ExtractorBoundaryError::Helper(error.to_string()))?;
    if !metadata.file_type().is_file() {
        return Err(ExtractorBoundaryError::Helper(
            "helper must be a regular file, not a symlink".to_string(),
        ));
    }
    Ok(path)
}

fn read_regular_bounded(path: &Path) -> Result<Vec<u8>, ExtractorBoundaryError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ExtractorBoundaryError::Artifact(error.to_string()))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_EXTRACTOR_OUTPUT_BYTES {
        return Err(ExtractorBoundaryError::Artifact(format!(
            "extractor output must be a regular file no larger than {MAX_EXTRACTOR_OUTPUT_BYTES} bytes"
        )));
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .map_err(|error| ExtractorBoundaryError::Artifact(error.to_string()))?,
    );
    File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|error| ExtractorBoundaryError::Artifact(error.to_string()))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != metadata.len() {
        return Err(ExtractorBoundaryError::Artifact(
            "extractor output changed while it was read".to_string(),
        ));
    }
    Ok(bytes)
}

struct PrivateRun {
    path: PathBuf,
}

impl PrivateRun {
    fn create() -> Result<Self, ExtractorBoundaryError> {
        for _attempt in 0..100 {
            let counter = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "texo-bvisor-extract-{}-{counter}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                            .map_err(|error| ExtractorBoundaryError::Artifact(error.to_string()))?;
                    }
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(ExtractorBoundaryError::Artifact(error.to_string())),
            }
        }
        Err(ExtractorBoundaryError::Artifact(
            "could not allocate private extractor directory".to_string(),
        ))
    }
}

impl Drop for PrivateRun {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_reader_rejects_non_regular_and_oversized_artifacts() {
        let run = PrivateRun::create().expect("private run");
        assert!(read_regular_bounded(&run.path).is_err());
        let output = run.path.join("large");
        let file = File::create(&output).expect("output");
        file.set_len(MAX_EXTRACTOR_OUTPUT_BYTES + 1)
            .expect("oversized output");
        assert!(read_regular_bounded(&output).is_err());
    }
}
