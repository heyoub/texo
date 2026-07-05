//! Workspace bootstrap: create `.texo/config.toml` on first run.
//!
//! The agent needs a texo workspace whose docs are the rendered session
//! transcripts. On startup, if the configured root has no `.texo/config.toml`,
//! one is written (the moral equivalent of `texo init`) with `docs_glob`
//! pointing at `sessions/**/*.md`, `extractor_cmd` pointing at the
//! `texo-extract` binary, and `[semantics] enabled = true` so the relate pass
//! is authoritative for supersession. An existing config is respected verbatim.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use texo_core::{SemanticsConfig, TexoRootConfig, WorkspaceEntry};

use crate::session::SESSIONS_DIR;

/// Environment variable pointing at the `texo-extract` binary.
pub const ENV_EXTRACT_BIN: &str = "TEXO_EXTRACT_BIN";
/// Environment variable selecting the record-once extract cache directory
/// (same convention as the justfile `demo-helios` recipe).
pub const ENV_EXTRACT_CACHE: &str = "TEXO_EXTRACT_CACHE";
/// Default extract cache directory, relative to the workspace root (matching
/// the `texo-extract` binary's own default name, anchored at the root).
pub const DEFAULT_EXTRACT_CACHE: &str = ".texo/extract-cache";

/// How the agent's workspace should be initialized on first run.
#[derive(Debug, Clone)]
pub struct BootstrapOptions {
    /// Workspace id written as the config's `default_workspace`.
    pub workspace_id: String,
    /// External extractor command for ingest (the `texo-extract` binary);
    /// `None` falls back to the built-in heuristic extractor.
    pub extractor_cmd: Option<String>,
    /// Write `[semantics] enabled = true` so `relate` owns supersession.
    pub semantics_enabled: bool,
}

impl Default for BootstrapOptions {
    fn default() -> Self {
        Self {
            workspace_id: "memory".to_owned(),
            extractor_cmd: None,
            semantics_enabled: true,
        }
    }
}

/// Create `.texo/config.toml` and the store directory unless they exist.
///
/// Idempotent: an existing config is never rewritten, so a hand-tuned
/// workspace (different models, thresholds, extractor) survives restarts.
pub fn ensure_workspace(root: &Path, options: &BootstrapOptions) -> Result<()> {
    let config_path = root.join(".texo").join("config.toml");
    if config_path.exists() {
        return Ok(());
    }

    let entry = WorkspaceEntry {
        store_path: ".texo/store".to_owned(),
        docs_glob: format!("{SESSIONS_DIR}/**/*.md"),
        extractor_cmd: options.extractor_cmd.clone(),
        semantics: options.semantics_enabled.then(|| SemanticsConfig {
            enabled: true,
            ..SemanticsConfig::default()
        }),
    };
    let mut workspaces = BTreeMap::new();
    workspaces.insert(options.workspace_id.clone(), entry);
    let config = TexoRootConfig {
        default_workspace: options.workspace_id.clone(),
        workspaces,
    };
    config
        .save(&config_path)
        .with_context(|| format!("writing {}", config_path.display()))?;
    std::fs::create_dir_all(root.join(".texo").join("store"))
        .context("creating store directory")?;
    Ok(())
}

/// Resolve the `texo-extract` binary path: `TEXO_EXTRACT_BIN` wins verbatim;
/// otherwise a `texo-extract` sibling of the current executable is used when it
/// exists. `None` means ingest falls back to the heuristic extractor.
pub fn resolve_extract_bin() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os(ENV_EXTRACT_BIN) {
        if !explicit.is_empty() {
            return Some(PathBuf::from(explicit));
        }
    }
    let exe = std::env::current_exe().ok()?;
    let sibling = exe.parent()?.join("texo-extract");
    sibling.exists().then_some(sibling)
}

/// Build the `extractor_cmd` shell string for the workspace config.
///
/// `extract_via_cmd` runs the command through `sh -c` with its cwd at the
/// *sessions* directory, so the record-once cache default must not be left
/// cwd-relative. The generated string honors an exported `TEXO_EXTRACT_CACHE`
/// (the justfile `demo-helios` convention) and otherwise anchors the cache at
/// the workspace root.
pub fn extractor_cmd_for(root: &Path, extract_bin: &Path) -> String {
    let cache = root.join(DEFAULT_EXTRACT_CACHE);
    format!(
        "{ENV_EXTRACT_CACHE}=\"${{{ENV_EXTRACT_CACHE}:-{}}}\" {}",
        cache.display(),
        extract_bin.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_writes_extractor_and_semantics() {
        let dir = tempfile::tempdir().expect("tempdir");
        let options = BootstrapOptions {
            workspace_id: "memory".to_owned(),
            extractor_cmd: Some("/opt/texo/texo-extract".to_owned()),
            semantics_enabled: true,
        };
        ensure_workspace(dir.path(), &options).expect("bootstrap");

        let config_path = dir.path().join(".texo").join("config.toml");
        let root_config = TexoRootConfig::load(&config_path).expect("load");
        assert_eq!(root_config.default_workspace, "memory");
        let ws = root_config.resolve(None).expect("resolve");
        assert_eq!(ws.docs_glob, "sessions/**/*.md");
        assert_eq!(ws.extractor_cmd.as_deref(), Some("/opt/texo/texo-extract"));
        assert!(ws.semantics.is_some_and(|s| s.enabled));
        assert!(dir.path().join(".texo").join("store").exists());
    }

    #[test]
    fn bootstrap_respects_an_existing_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        ensure_workspace(dir.path(), &BootstrapOptions::default()).expect("first");
        let config_path = dir.path().join(".texo").join("config.toml");
        let before = std::fs::read_to_string(&config_path).expect("read");

        // A second bootstrap with different options must not clobber the file.
        let other = BootstrapOptions {
            workspace_id: "other".to_owned(),
            extractor_cmd: Some("/elsewhere/texo-extract".to_owned()),
            semantics_enabled: false,
        };
        ensure_workspace(dir.path(), &other).expect("second");
        let after = std::fs::read_to_string(&config_path).expect("read");
        assert_eq!(before, after, "existing config must be respected verbatim");
    }

    #[test]
    fn heuristic_bootstrap_omits_extractor_and_semantics() {
        let dir = tempfile::tempdir().expect("tempdir");
        let options = BootstrapOptions {
            workspace_id: "memory".to_owned(),
            extractor_cmd: None,
            semantics_enabled: false,
        };
        ensure_workspace(dir.path(), &options).expect("bootstrap");
        let ws = TexoRootConfig::load(&dir.path().join(".texo").join("config.toml"))
            .expect("load")
            .resolve(None)
            .expect("resolve");
        assert_eq!(ws.extractor_cmd, None);
        assert_eq!(ws.semantics, None);
    }

    #[test]
    fn extractor_cmd_honors_exported_cache_and_anchors_default_at_root() {
        let cmd = extractor_cmd_for(Path::new("/srv/agent"), Path::new("/opt/texo-extract"));
        assert_eq!(
            cmd,
            "TEXO_EXTRACT_CACHE=\"${TEXO_EXTRACT_CACHE:-/srv/agent/.texo/extract-cache}\" /opt/texo-extract"
        );
    }
}
