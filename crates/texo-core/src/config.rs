//! Workspace configuration persisted under `.texo/config.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::ids::WorkspaceId;

/// Resolved configuration for one BatPak workspace scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    /// Workspace identifier for BatPak scope partitioning.
    pub workspace_id: String,
    /// Relative or absolute path to the BatPak store directory.
    pub store_path: String,
    /// Glob for default markdown sources.
    pub docs_glob: String,
    /// Optional external extractor command (newline-delimited JSON claims).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extractor_cmd: Option<String>,
}

/// Backward-compatible alias for [`WorkspaceConfig`].
pub type TexoConfig = WorkspaceConfig;

/// Per-workspace entry in the root config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceEntry {
    /// Relative or absolute path to the BatPak store directory.
    pub store_path: String,
    /// Glob for default markdown sources.
    pub docs_glob: String,
    /// Optional external extractor command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extractor_cmd: Option<String>,
}

/// Root `.texo/config.toml` with multiple workspace scopes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TexoRootConfig {
    /// Default workspace id when none is specified on the CLI.
    pub default_workspace: String,
    /// Named workspace configurations.
    #[serde(default)]
    pub workspaces: BTreeMap<String, WorkspaceEntry>,
}

/// Legacy flat config shape (pre-v1).
#[derive(Debug, Deserialize)]
struct LegacyFlatConfig {
    workspace_id: String,
    store_path: String,
    docs_glob: String,
    extractor_cmd: Option<String>,
}

impl WorkspaceEntry {
    /// Defaults for the demo workspace.
    pub fn demo() -> Self {
        Self {
            store_path: crate::fixture::DEFAULT_STORE_PATH.to_string(),
            docs_glob: "sample_sources/**/*.md".to_string(),
            extractor_cmd: None,
        }
    }

    /// Defaults for a secondary workspace with an isolated store.
    pub fn for_id(workspace_id: &str) -> Self {
        if workspace_id == crate::fixture::DEFAULT_WORKSPACE_ID {
            return Self::demo();
        }
        Self {
            store_path: format!(".texo/stores/{workspace_id}"),
            docs_glob: "sample_sources/**/*.md".to_string(),
            extractor_cmd: None,
        }
    }
}

impl TexoRootConfig {
    /// Default root config with only the demo workspace.
    pub fn demo() -> Self {
        let mut workspaces = BTreeMap::new();
        workspaces.insert(
            crate::fixture::DEFAULT_WORKSPACE_ID.to_string(),
            WorkspaceEntry::demo(),
        );
        Self {
            default_workspace: crate::fixture::DEFAULT_WORKSPACE_ID.to_string(),
            workspaces,
        }
    }

    /// Load configuration from a TOML file (legacy flat or nested workspaces).
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
        if let Ok(legacy) = toml::from_str::<LegacyFlatConfig>(&raw) {
            let mut workspaces = BTreeMap::new();
            workspaces.insert(
                legacy.workspace_id.clone(),
                WorkspaceEntry {
                    store_path: legacy.store_path,
                    docs_glob: legacy.docs_glob,
                    extractor_cmd: legacy.extractor_cmd,
                },
            );
            return Ok(Self {
                default_workspace: legacy.workspace_id,
                workspaces,
            });
        }
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

    /// Resolve a workspace by id or fall back to the default.
    pub fn resolve(&self, workspace_id: Option<&str>) -> Result<WorkspaceConfig, ConfigError> {
        let id = workspace_id.unwrap_or(self.default_workspace.as_str());
        let entry = self
            .workspaces
            .get(id)
            .ok_or_else(|| ConfigError::UnknownWorkspace(id.to_string()))?;
        Ok(WorkspaceConfig {
            workspace_id: id.to_string(),
            store_path: entry.store_path.clone(),
            docs_glob: entry.docs_glob.clone(),
            extractor_cmd: entry.extractor_cmd.clone(),
        })
    }

    /// Insert or update a workspace entry and set it as default when first.
    pub fn upsert_workspace(&mut self, workspace_id: &str, entry: WorkspaceEntry) {
        if self.workspaces.is_empty() {
            let id = workspace_id.to_string();
            self.default_workspace.clone_from(&id);
        }
        self.workspaces.insert(workspace_id.to_string(), entry);
    }
}

impl WorkspaceConfig {
    /// Default configuration for the demo workspace.
    pub fn demo() -> Self {
        Self {
            workspace_id: crate::fixture::DEFAULT_WORKSPACE_ID.to_string(),
            store_path: crate::fixture::DEFAULT_STORE_PATH.to_string(),
            docs_glob: "sample_sources/**/*.md".to_string(),
            extractor_cmd: None,
        }
    }

    /// Load a single workspace from a legacy flat config file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        TexoRootConfig::load(path)?.resolve(None)
    }

    /// Write a legacy flat config file containing only this workspace.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let mut root = TexoRootConfig::demo();
        root.default_workspace.clone_from(&self.workspace_id);
        root.workspaces.insert(
            self.workspace_id.clone(),
            WorkspaceEntry {
                store_path: self.store_path.clone(),
                docs_glob: self.docs_glob.clone(),
                extractor_cmd: self.extractor_cmd.clone(),
            },
        );
        root.save(path)
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

    /// Resolve the directory that [`Self::docs_glob`] scans, relative to `root`.
    ///
    /// The returned path is the literal (non-wildcard) prefix of the configured
    /// glob, which is the directory tree the staleness scan should walk. A glob
    /// such as `docs/**/*.md` resolves to `<root>/docs`, while `sample_sources/**/*.md`
    /// resolves to `<root>/sample_sources`. Patterns whose first component is
    /// already a wildcard resolve to `root` itself.
    pub fn docs_scan_root(&self, root: &Path) -> PathBuf {
        let glob_path = PathBuf::from(&self.docs_glob);
        let mut prefix = PathBuf::new();
        for component in glob_path.components() {
            let part = component.as_os_str().to_string_lossy();
            if part.contains(['*', '?', '[']) {
                break;
            }
            prefix.push(component);
        }
        if prefix.as_os_str().is_empty() {
            return root.to_path_buf();
        }
        if prefix.is_absolute() {
            prefix
        } else {
            root.join(prefix)
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
    /// Unknown workspace id in root config.
    #[error("unknown workspace: {0}")]
    UnknownWorkspace(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_flat_config_loads_as_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
workspace_id = "demo"
store_path = ".texo/store"
docs_glob = "sample_sources/**/*.md"
"#,
        )
        .expect("write");

        let root = TexoRootConfig::load(&path).expect("load");
        assert_eq!(root.default_workspace, "demo");
        let ws = root.resolve(None).expect("resolve");
        assert_eq!(ws.workspace_id, "demo");
    }

    #[test]
    fn nested_config_resolves_staging() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
default_workspace = "demo"

[workspaces.demo]
store_path = ".texo/store"
docs_glob = "sample_sources/**/*.md"

[workspaces.staging]
store_path = ".texo/stores/staging"
docs_glob = "docs/**/*.md"
"#,
        )
        .expect("write");

        let root = TexoRootConfig::load(&path).expect("load");
        let staging = root.resolve(Some("staging")).expect("staging");
        assert_eq!(staging.store_path, ".texo/stores/staging");
    }

    #[test]
    fn docs_scan_root_uses_glob_prefix() {
        let root = Path::new("/ws");

        let demo = WorkspaceConfig::demo();
        assert_eq!(demo.docs_scan_root(root), root.join("sample_sources"));

        let staging = WorkspaceConfig {
            workspace_id: "staging".to_string(),
            store_path: ".texo/stores/staging".to_string(),
            docs_glob: "docs/**/*.md".to_string(),
            extractor_cmd: None,
        };
        assert_eq!(staging.docs_scan_root(root), root.join("docs"));
    }

    #[test]
    fn docs_scan_root_falls_back_to_root_for_leading_wildcard() {
        let root = Path::new("/ws");
        let cfg = WorkspaceConfig {
            workspace_id: "demo".to_string(),
            store_path: ".texo/store".to_string(),
            docs_glob: "**/*.md".to_string(),
            extractor_cmd: None,
        };
        assert_eq!(cfg.docs_scan_root(root), root.to_path_buf());
    }
}
