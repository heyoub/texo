//! Workspace configuration persisted under `.texo/config.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::ids::WorkspaceId;

/// Default cosine-similarity acceptance threshold for the semantics pipeline.
///
/// Used by `texo relate` as the **cluster link threshold** for connected-component
/// candidate generation (see `texo-core`'s `semantics_pipeline`): claims are
/// clustered at this similarity and the LLM judge only sees within-cluster pairs.
/// It must sit at or below the corpus's lowest same-subject similarity so no true
/// pair is split across clusters — measured on the Helios corpus with the hosted
/// embedder, the floor is Postgres↔BatPak ≈ 0.70, hence 0.65 (margin below the
/// floor, above the 0.60 relate prefilter). Raise it per workspace to prune more
/// aggressively on corpora whose subjects separate more cleanly.
const DEFAULT_COSINE_THRESHOLD: f32 = 0.65;

fn default_cosine_threshold() -> f32 {
    DEFAULT_COSINE_THRESHOLD
}

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
    /// Pinned revision/identifier for the embedding model.
    #[serde(default)]
    pub embed_model_revision: String,
    /// Pinned revision/identifier for the NLI model.
    #[serde(default)]
    pub nli_model_revision: String,
    /// Cosine-similarity acceptance threshold; `texo relate` uses it as the
    /// cluster link threshold for candidate generation. Must sit at or below the
    /// corpus's lowest same-subject similarity (default 0.65; see the module
    /// source for the measured rationale).
    #[serde(default = "default_cosine_threshold")]
    pub cosine_threshold: f32,
    /// Optional override for the semantic extractor model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extractor_model: Option<String>,
}

impl Default for SemanticsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            embed_model_revision: String::new(),
            nli_model_revision: String::new(),
            cosine_threshold: DEFAULT_COSINE_THRESHOLD,
            extractor_model: None,
        }
    }
}

/// Resolved configuration for one BatPak workspace scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Optional, disabled-by-default semantic ML pipeline configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantics: Option<SemanticsConfig>,
}

/// Backward-compatible alias for [`WorkspaceConfig`].
pub type TexoConfig = WorkspaceConfig;

/// Per-workspace entry in the root config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceEntry {
    /// Relative or absolute path to the BatPak store directory.
    pub store_path: String,
    /// Glob for default markdown sources.
    pub docs_glob: String,
    /// Optional external extractor command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extractor_cmd: Option<String>,
    /// Optional, disabled-by-default semantic ML pipeline configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantics: Option<SemanticsConfig>,
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
}

/// Legacy flat config shape (pre-v1).
#[derive(Debug, Deserialize)]
struct LegacyFlatConfig {
    workspace_id: String,
    store_path: String,
    docs_glob: String,
    extractor_cmd: Option<String>,
    #[serde(default)]
    semantics: Option<SemanticsConfig>,
}

impl WorkspaceEntry {
    /// Defaults for the demo workspace.
    pub fn demo() -> Self {
        Self {
            store_path: crate::fixture::DEFAULT_STORE_PATH.to_string(),
            docs_glob: "sample_sources/**/*.md".to_string(),
            extractor_cmd: None,
            semantics: None,
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
            semantics: None,
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
                    semantics: legacy.semantics,
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
            semantics: entry.semantics.clone(),
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
            semantics: None,
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
                semantics: self.semantics.clone(),
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
    use assert_matches::assert_matches;

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
            semantics: None,
        };
        assert_eq!(staging.docs_scan_root(root), root.join("docs"));
    }

    #[test]
    fn workspace_config_load_save_roundtrip() {
        // WorkspaceConfig::save writes a legacy-flat file (creating the parent
        // dir); WorkspaceConfig::load reads it back as the default workspace.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/dir/config.toml");
        let cfg = WorkspaceConfig {
            workspace_id: "staging".to_string(),
            store_path: ".texo/stores/staging".to_string(),
            docs_glob: "docs/**/*.md".to_string(),
            extractor_cmd: Some("./extract.sh".to_string()),
            semantics: None,
        };
        cfg.save(&path).expect("save");
        assert!(path.exists(), "save must create the parent directories");

        let loaded = WorkspaceConfig::load(&path).expect("load");
        assert_eq!(loaded.workspace_id, "staging");
        assert_eq!(loaded.store_path, ".texo/stores/staging");
        assert_eq!(loaded.docs_glob, "docs/**/*.md");
        assert_eq!(loaded.extractor_cmd.as_deref(), Some("./extract.sh"));
    }

    #[test]
    fn workspace_parses_valid_id_and_rejects_invalid() {
        let cfg = WorkspaceConfig::demo();
        let id = cfg.workspace().expect("valid id");
        assert_eq!(id.as_str(), crate::fixture::DEFAULT_WORKSPACE_ID);

        let bad = WorkspaceConfig {
            // A slash is rejected by workspace-id validation (path-injection guard).
            workspace_id: "bad/workspace".to_string(),
            store_path: ".texo/store".to_string(),
            docs_glob: "**/*.md".to_string(),
            extractor_cmd: None,
            semantics: None,
        };
        assert_matches!(bad.workspace(), Err(ConfigError::InvalidWorkspace));
    }

    #[test]
    fn store_path_buf_respects_absolute_and_relative() {
        let cfg_rel = WorkspaceConfig::demo();
        assert_eq!(
            cfg_rel.store_path_buf(Path::new("/ws")),
            Path::new("/ws").join(crate::fixture::DEFAULT_STORE_PATH)
        );

        let cfg_abs = WorkspaceConfig {
            workspace_id: "demo".to_string(),
            store_path: "/abs/store".to_string(),
            docs_glob: "**/*.md".to_string(),
            extractor_cmd: None,
            semantics: None,
        };
        // Absolute store paths ignore the root and are returned verbatim.
        assert_eq!(
            cfg_abs.store_path_buf(Path::new("/ws")),
            PathBuf::from("/abs/store")
        );
    }

    #[test]
    fn docs_scan_root_returns_absolute_prefix_verbatim() {
        let cfg = WorkspaceConfig {
            workspace_id: "demo".to_string(),
            store_path: ".texo/store".to_string(),
            docs_glob: "/srv/docs/**/*.md".to_string(),
            extractor_cmd: None,
            semantics: None,
        };
        // An absolute non-wildcard prefix is returned without joining root.
        assert_eq!(
            cfg.docs_scan_root(Path::new("/ws")),
            PathBuf::from("/srv/docs")
        );
    }

    #[test]
    fn upsert_first_workspace_becomes_default() {
        let mut root = TexoRootConfig {
            default_workspace: "placeholder".to_string(),
            workspaces: BTreeMap::new(),
        };
        // First upsert into an empty map promotes the id to the default.
        root.upsert_workspace("alpha", WorkspaceEntry::for_id("alpha"));
        assert_eq!(root.default_workspace, "alpha");

        // A subsequent upsert does NOT change the default.
        root.upsert_workspace("beta", WorkspaceEntry::for_id("beta"));
        assert_eq!(root.default_workspace, "alpha");
        assert!(root.workspaces.contains_key("beta"));
    }

    #[test]
    fn for_id_demo_matches_demo_entry() {
        assert_eq!(
            WorkspaceEntry::for_id(crate::fixture::DEFAULT_WORKSPACE_ID),
            WorkspaceEntry::demo()
        );
        let other = WorkspaceEntry::for_id("staging");
        assert_eq!(other.store_path, ".texo/stores/staging");
    }

    #[test]
    fn save_into_unmakeable_parent_is_io_error() {
        // Place a regular FILE where save() needs a directory; create_dir_all then
        // fails with a real filesystem IO error that must surface as ConfigError::Io.
        let dir = tempfile::tempdir().expect("tempdir");
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"i am a file, not a dir").expect("write blocker");
        // `blocker/sub/config.toml` cannot be created because `blocker` is a file.
        let path = blocker.join("sub").join("config.toml");
        assert_matches!(TexoRootConfig::demo().save(&path), Err(ConfigError::Io(_)));
    }

    #[test]
    fn load_nonexistent_path_is_io_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope.toml");
        assert_matches!(TexoRootConfig::load(&missing), Err(ConfigError::Io(_)));
    }

    #[test]
    fn load_directory_instead_of_file_is_io_error() {
        // Pointing load at a directory makes read_to_string fail with an IO error
        // (not a parse error).
        let dir = tempfile::tempdir().expect("tempdir");
        assert_matches!(TexoRootConfig::load(dir.path()), Err(ConfigError::Io(_)));
    }

    #[test]
    fn load_garbage_toml_is_parse_error() {
        // Bytes that are neither a valid legacy-flat config nor a valid nested
        // config must surface as a Parse error (after the legacy attempt fails).
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, b"this is = = not valid toml {{{").expect("write");
        assert_matches!(TexoRootConfig::load(&path), Err(ConfigError::Parse(_)));
    }

    #[test]
    fn load_well_formed_but_wrong_shape_is_parse_error() {
        // Valid TOML that satisfies neither shape (missing required fields, and
        // unknown fields under deny_unknown_fields) must be a Parse error.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, b"unrelated_key = 42\n").expect("write");
        assert_matches!(TexoRootConfig::load(&path), Err(ConfigError::Parse(_)));
    }

    #[test]
    fn resolve_unknown_workspace_errors() {
        let root = TexoRootConfig::demo();
        assert_matches!(
            root.resolve(Some("ghost")),
            Err(ConfigError::UnknownWorkspace(id)) if id == "ghost"
        );
    }

    #[test]
    fn resolve_default_when_none_requested() {
        let root = TexoRootConfig::demo();
        let ws = root.resolve(None).expect("default resolves");
        assert_eq!(ws.workspace_id, crate::fixture::DEFAULT_WORKSPACE_ID);
    }

    #[test]
    fn docs_scan_root_falls_back_to_root_for_leading_wildcard() {
        let root = Path::new("/ws");
        let cfg = WorkspaceConfig {
            workspace_id: "demo".to_string(),
            store_path: ".texo/store".to_string(),
            docs_glob: "**/*.md".to_string(),
            extractor_cmd: None,
            semantics: None,
        };
        assert_eq!(cfg.docs_scan_root(root), root.to_path_buf());
    }
}
