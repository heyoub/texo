use std::time::Duration;

use crate::knowledge::CodeIndexArtifact;

/// Bounds for one code-index build/import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeIndexLimits {
    /// Maximum raw SCIP bytes accepted.
    pub max_scip_bytes: u64,
    /// Maximum documents consumed.
    pub max_documents: usize,
    /// Maximum normalized occurrences retained.
    pub max_occurrences: usize,
    /// Global wall budget for built-in analysis.
    pub analysis_budget: Duration,
}

impl Default for CodeIndexLimits {
    fn default() -> Self {
        Self {
            max_scip_bytes: 64 * 1024 * 1024,
            max_documents: 20_000,
            max_occurrences: 200_000,
            analysis_budget: Duration::from_secs(30),
        }
    }
}

/// A built artifact and the digest of its serialized bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCodeIndex {
    /// Normalized disposable artifact.
    pub artifact: CodeIndexArtifact,
    /// BLAKE3 digest of the exact persisted artifact bytes.
    pub artifact_digest_hex: String,
    /// Serialized artifact bytes.
    pub bytes: Vec<u8>,
}
