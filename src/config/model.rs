//! Persisted configuration shapes and failures.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{default_cosine_threshold, WorkspaceEntry, DEFAULT_COSINE_THRESHOLD};
use crate::gateway::GatewayConfig;

/// Optional, disabled-by-default configuration for the semantic ML pipeline.
///
/// The semantic pipeline is entirely opt-in: a workspace config without a
/// `[semantics]` table deserializes to `None`, and even when present the
/// pipeline only activates when [`SemanticsConfig::enabled`] is `true`. No ML
/// or model-runtime behavior lives here — this is configuration only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticsConfig {
    /// Master switch; when `false` (the default) the pipeline is inert.
    #[serde(default)]
    pub enabled: bool,
    /// Cosine-similarity acceptance threshold.
    #[serde(default = "default_cosine_threshold")]
    pub cosine_threshold: f32,
    /// Within-cluster pair prefilter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relate_prefilter: Option<f32>,
}

impl Default for SemanticsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cosine_threshold: DEFAULT_COSINE_THRESHOLD,
            relate_prefilter: None,
        }
    }
}

/// Resolved configuration for one `BatPak` workspace scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    /// Workspace identifier for `BatPak` scope partitioning.
    pub workspace_id: String,
    /// Relative or absolute path to the `BatPak` store directory.
    pub store_path: String,
    /// Glob for default markdown sources.
    pub docs_glob: String,
    /// Optional external extractor command (newline-delimited JSON claims).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extractor_cmd: Option<String>,
    /// Optional, disabled-by-default semantic ML pipeline configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantics: Option<SemanticsConfig>,
    /// Optional process-wide model gateway configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<GatewayConfig>,
}

/// Root `.texo/config.toml` with multiple workspace scopes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TexoRootConfig {
    /// Default workspace id when none is specified on the CLI.
    pub default_workspace: String,
    /// Named workspace configurations.
    #[serde(default)]
    pub workspaces: BTreeMap<String, WorkspaceEntry>,
    /// Optional model gateway configuration. Bootstrap never writes this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<GatewayConfig>,
}

/// Configuration-specific failures.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Filesystem error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// TOML parse error.
    #[error("parse: {0}")]
    Parse(#[from] toml::de::Error),
    /// TOML serialize error.
    #[error("serialize: {0}")]
    Serialize(#[from] toml::ser::Error),
    /// Invalid workspace identifier.
    #[error("invalid workspace id")]
    InvalidWorkspace,
    /// Unknown workspace id in root config.
    #[error("unknown workspace: {0}")]
    UnknownWorkspace(String),
    /// Invalid journal topology or selection.
    #[error("topology: {0}")]
    Topology(#[from] crate::topology::TopologyError),
}
