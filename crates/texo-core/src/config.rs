//! Workspace configuration persisted under `.texo/config.toml`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::ids::WorkspaceId;
/// On-disk texo configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TexoConfig {
    /// Workspace identifier for BatPak scope partitioning.
    pub workspace_id: String,
    /// Relative or absolute path to the BatPak store directory.
    pub store_path: String,
    /// Glob for default markdown sources.
    pub docs_glob: String,
}

impl TexoConfig {
    /// Default configuration for the demo workspace.
    pub fn demo() -> Self {
        Self {
            workspace_id: crate::fixture::DEFAULT_WORKSPACE_ID.to_string(),
            store_path: crate::fixture::DEFAULT_STORE_PATH.to_string(),
            docs_glob: "sample_sources/**/*.md".to_string(),
        }
    }

    /// Load configuration from a TOML file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
        toml::from_str(&raw).map_err(ConfigError::Parse)
    }

    /// Write configuration to a TOML file.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ConfigError::Io)?;
        }
        let raw = toml::to_string_pretty(self).map_err(ConfigError::Serialize)?;
        std::fs::write(path, raw).map_err(ConfigError::Io)
    }

    /// Parse the configured workspace id.
    pub fn workspace(&self) -> Result<WorkspaceId, ConfigError> {
        WorkspaceId::try_from(self.workspace_id.as_str()).map_err(|_| ConfigError::InvalidWorkspace)
    }

    /// Resolve store path relative to a workspace root.
    pub fn store_path_buf(&self, root: &Path) -> PathBuf {
        let path = PathBuf::from(&self.store_path);
        if path.is_absolute() {
            path
        } else {
            root.join(path)
        }
    }
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
}
