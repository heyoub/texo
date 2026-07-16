//! Workspace configuration persisted under `.texo/config.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::events::ids::WorkspaceId;
use crate::topology::{self, JournalEntry, ResolvedJournal};

mod model;

pub use model::{ConfigError, SemanticsConfig, TexoRootConfig, WorkspaceConfig};

const DEFAULT_WORKSPACE_ID: &str = "demo";
const DEFAULT_STORE_PATH: &str = ".texo/store";

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

/// Per-workspace entry in the root config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceEntry {
    /// Default authoritative journal selected when no journal is requested.
    pub primary_journal: String,
    /// Normalized physical journals keyed by stable workspace-local id.
    pub journals: BTreeMap<String, JournalEntry>,
    /// Glob for default markdown sources.
    pub docs_glob: String,
    /// Optional external extractor command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extractor_cmd: Option<String>,
    /// Optional, disabled-by-default semantic ML pipeline configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantics: Option<SemanticsConfig>,
}

impl WorkspaceEntry {
    /// Defaults for the demo workspace.
    #[must_use]
    pub fn demo() -> Self {
        let mut journals = BTreeMap::new();
        journals.insert(
            "canonical".to_string(),
            JournalEntry::canonical(DEFAULT_STORE_PATH),
        );
        Self {
            primary_journal: "canonical".to_string(),
            journals,
            docs_glob: "sample_sources/**/*.md".to_string(),
            extractor_cmd: None,
            semantics: None,
        }
    }

    /// Defaults for a secondary workspace with an isolated store.
    #[must_use]
    pub fn for_id(workspace_id: &str) -> Self {
        if workspace_id == DEFAULT_WORKSPACE_ID {
            return Self::demo();
        }
        let mut journals = BTreeMap::new();
        journals.insert(
            "canonical".to_string(),
            JournalEntry::canonical(format!(".texo/stores/{workspace_id}")),
        );
        Self {
            primary_journal: "canonical".to_string(),
            journals,
            docs_glob: "sample_sources/**/*.md".to_string(),
            extractor_cmd: None,
            semantics: None,
        }
    }

    /// Resolve the default canonical journal declaration.
    ///
    /// # Errors
    /// Returns [`ConfigError::Topology`] when the topology is invalid.
    pub fn primary(&self) -> Result<ResolvedJournal, ConfigError> {
        topology::resolve_journal(&self.primary_journal, &self.journals, None)
            .map_err(ConfigError::from)
    }

    /// Replace only the default canonical journal's physical path.
    ///
    /// # Errors
    /// Returns [`ConfigError::Topology`] when the primary declaration is absent
    /// or invalid.
    pub fn set_primary_store_path(
        &mut self,
        store_path: impl Into<String>,
    ) -> Result<(), ConfigError> {
        let primary = self.primary()?;
        let entry = self.journals.get_mut(primary.id.as_str()).ok_or_else(|| {
            ConfigError::Topology(crate::topology::TopologyError::MissingJournal(
                primary.id.to_string(),
            ))
        })?;
        entry.store_path = store_path.into();
        Ok(())
    }
}

impl TexoRootConfig {
    /// Default root config with only the demo workspace.
    #[must_use]
    pub fn demo() -> Self {
        let mut workspaces = BTreeMap::new();
        workspaces.insert(DEFAULT_WORKSPACE_ID.to_string(), WorkspaceEntry::demo());
        Self {
            default_workspace: DEFAULT_WORKSPACE_ID.to_string(),
            workspaces,
            gateway: None,
        }
    }

    /// Load configuration from a TOML file.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] when the config file cannot be read;
    /// [`ConfigError::Parse`] when it does not match the current root shape.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
        toml::from_str(&raw).map_err(ConfigError::Parse)
    }

    /// Write configuration to a TOML file.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Serialize`] when the config cannot be rendered as
    /// TOML; [`ConfigError::Io`] when the parent directory cannot be created or
    /// the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ConfigError::Io)?;
        }
        let raw = toml::to_string_pretty(self).map_err(ConfigError::Serialize)?;
        std::fs::write(path, raw).map_err(ConfigError::Io)
    }

    /// Resolve a workspace by id or fall back to the default.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::UnknownWorkspace`] when the requested (or default)
    /// id has no entry in [`Self::workspaces`].
    pub fn resolve(&self, workspace_id: Option<&str>) -> Result<WorkspaceConfig, ConfigError> {
        self.resolve_journal(workspace_id, None)
            .map(|(workspace, _journal)| workspace)
    }

    /// Resolve a workspace and one selected physical journal.
    ///
    /// # Errors
    /// Returns [`ConfigError::UnknownWorkspace`] or [`ConfigError::Topology`]
    /// when the normalized topology cannot resolve the selection.
    pub fn resolve_journal(
        &self,
        workspace_id: Option<&str>,
        journal_id: Option<&str>,
    ) -> Result<(WorkspaceConfig, ResolvedJournal), ConfigError> {
        let id = workspace_id.unwrap_or(self.default_workspace.as_str());
        let entry = self
            .workspaces
            .get(id)
            .ok_or_else(|| ConfigError::UnknownWorkspace(id.to_string()))?;
        let journal =
            topology::resolve_journal(&entry.primary_journal, &entry.journals, journal_id)?;
        let workspace = WorkspaceConfig {
            workspace_id: id.to_string(),
            store_path: journal.store_path.clone(),
            docs_glob: entry.docs_glob.clone(),
            extractor_cmd: entry.extractor_cmd.clone(),
            semantics: entry.semantics.clone(),
            gateway: self.gateway.clone(),
        };
        Ok((workspace, journal))
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
    #[must_use]
    pub fn demo() -> Self {
        Self {
            workspace_id: DEFAULT_WORKSPACE_ID.to_string(),
            store_path: DEFAULT_STORE_PATH.to_string(),
            docs_glob: "sample_sources/**/*.md".to_string(),
            extractor_cmd: None,
            semantics: None,
            gateway: None,
        }
    }

    /// Load a single workspace from a legacy flat config file.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] or [`ConfigError::Parse`] when
    /// [`TexoRootConfig::load`] fails; [`ConfigError::UnknownWorkspace`] when the
    /// file's default workspace has no entry.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        TexoRootConfig::load(path)?.resolve(None)
    }

    /// Write a legacy flat config file containing only this workspace.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Serialize`] when the config cannot be rendered as
    /// TOML; [`ConfigError::Io`] when the parent directory cannot be created or
    /// the file cannot be written (via [`TexoRootConfig::save`]).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let mut root = TexoRootConfig::demo();
        root.default_workspace.clone_from(&self.workspace_id);
        root.workspaces.insert(
            self.workspace_id.clone(),
            WorkspaceEntry {
                primary_journal: "canonical".to_string(),
                journals: BTreeMap::from([(
                    "canonical".to_string(),
                    JournalEntry::canonical(self.store_path.clone()),
                )]),
                docs_glob: self.docs_glob.clone(),
                extractor_cmd: self.extractor_cmd.clone(),
                semantics: self.semantics.clone(),
            },
        );
        root.gateway.clone_from(&self.gateway);
        root.save(path)
    }

    /// Parse the configured workspace id.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidWorkspace`] when `workspace_id` is empty or
    /// contains a path-unsafe character (`/`, `\`, or NUL).
    pub fn workspace(&self) -> Result<WorkspaceId, ConfigError> {
        WorkspaceId::try_from(self.workspace_id.as_str()).map_err(|_| ConfigError::InvalidWorkspace)
    }

    /// Resolve store path relative to a workspace root.
    #[must_use]
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
    #[must_use]
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

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn nested_config_resolves_staging() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
default_workspace = "demo"

[workspaces.demo]
primary_journal = "canonical"
docs_glob = "sample_sources/**/*.md"

[workspaces.demo.journals.canonical]
role = "canonical"
store_path = ".texo/store"

[workspaces.staging]
primary_journal = "canonical"
docs_glob = "docs/**/*.md"

[workspaces.staging.journals.canonical]
role = "canonical"
store_path = ".texo/stores/staging"
"#,
        )
        .expect("write");

        let root = TexoRootConfig::load(&path).expect("load");
        let staging = root.resolve(Some("staging")).expect("staging");
        assert_eq!(staging.store_path, ".texo/stores/staging");
    }

    #[test]
    fn gateway_is_read_when_present_and_omitted_from_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
default_workspace = "demo"

[workspaces.demo]
primary_journal = "canonical"
docs_glob = "docs/**/*.md"

[workspaces.demo.journals.canonical]
role = "canonical"
store_path = ".texo/store"

[gateway.providers.dashscope]
base_url = "https://dashscope.example/v1"
api_key_env = "TEXO_LLM_API_KEY"
embed_batch_max = 10
strict_json_schema_ok = false
expects_reasoning = true
retry_max = 2
request_timeout_secs = 30

[gateway.relate]
provider = "dashscope"
model = "qwen"
max_completion_tokens = 4096
temperature = 0.0
response_format = "json_schema"
"#,
        )
        .expect("write");
        let loaded = TexoRootConfig::load(&path).expect("load");
        let gateway = loaded.gateway.as_ref().expect("gateway");
        assert_eq!(gateway.relate.as_ref().expect("relate").model, "qwen");
        assert_eq!(gateway.providers["dashscope"].embed_batch_max, 10);

        let default_toml = toml::to_string(&TexoRootConfig::demo()).expect("serialize");
        assert!(!default_toml.contains("[gateway]"));
        assert!(!default_toml.contains("[gateway."));
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
            gateway: None,
        };
        assert_eq!(staging.docs_scan_root(root), root.join("docs"));
    }

    #[test]
    fn workspace_config_load_save_roundtrip() {
        // WorkspaceConfig::save writes the current root shape and creates the
        // parent directory; WorkspaceConfig::load resolves its default workspace.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/dir/config.toml");
        let cfg = WorkspaceConfig {
            workspace_id: "staging".to_string(),
            store_path: ".texo/stores/staging".to_string(),
            docs_glob: "docs/**/*.md".to_string(),
            extractor_cmd: Some("./extract.sh".to_string()),
            semantics: None,
            gateway: None,
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
        assert_eq!(id.as_str(), DEFAULT_WORKSPACE_ID);

        let bad = WorkspaceConfig {
            // A slash is rejected by workspace-id validation (path-injection guard).
            workspace_id: "bad/workspace".to_string(),
            store_path: ".texo/store".to_string(),
            docs_glob: "**/*.md".to_string(),
            extractor_cmd: None,
            semantics: None,
            gateway: None,
        };
        assert_matches!(bad.workspace(), Err(ConfigError::InvalidWorkspace));
    }

    #[test]
    fn store_path_buf_respects_absolute_and_relative() {
        let cfg_rel = WorkspaceConfig::demo();
        assert_eq!(
            cfg_rel.store_path_buf(Path::new("/ws")),
            Path::new("/ws").join(DEFAULT_STORE_PATH)
        );

        let cfg_abs = WorkspaceConfig {
            workspace_id: "demo".to_string(),
            store_path: "/abs/store".to_string(),
            docs_glob: "**/*.md".to_string(),
            extractor_cmd: None,
            semantics: None,
            gateway: None,
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
            gateway: None,
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
            gateway: None,
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
            WorkspaceEntry::for_id(DEFAULT_WORKSPACE_ID),
            WorkspaceEntry::demo()
        );
        let other = WorkspaceEntry::for_id("staging");
        assert_eq!(
            other.primary().expect("primary").store_path,
            ".texo/stores/staging"
        );
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
        // Invalid current-shape TOML must surface as a parse error.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, b"this is = = not valid toml {{{").expect("write");
        assert_matches!(TexoRootConfig::load(&path), Err(ConfigError::Parse(_)));
    }

    #[test]
    fn load_well_formed_but_wrong_shape_is_parse_error() {
        // Unknown fields under deny_unknown_fields must be a parse error.
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
        assert_eq!(ws.workspace_id, DEFAULT_WORKSPACE_ID);
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
            gateway: None,
        };
        assert_eq!(cfg.docs_scan_root(root), root.to_path_buf());
    }
}
