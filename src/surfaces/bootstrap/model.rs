//! Bootstrap input and decision shapes.

use std::path::PathBuf;

/// Inputs to extractor resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapInputs {
    /// `TEXO_EXTRACT_BIN` value, when present.
    pub extract_bin: Option<String>,
    /// Resolved neutral model API-key value, when present.
    pub model_api_key: Option<String>,
    /// Current executable path.
    pub current_exe: PathBuf,
}

/// Extractor resolution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapDecision {
    /// Shell command written to config, or `None` for heuristic extraction.
    pub extractor_cmd: Option<String>,
    /// Whether `[semantics] enabled = true` should be written.
    pub semantics_enabled: bool,
    /// Optional startup warning.
    pub warning: Option<String>,
}
