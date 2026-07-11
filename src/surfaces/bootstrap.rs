//! First-run workspace bootstrap for the memory-agent surface.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::{SemanticsConfig, TexoRootConfig, WorkspaceEntry};
use crate::error::TexoError;

/// Environment variable pointing at an extractor binary.
pub const ENV_EXTRACT_BIN: &str = "TEXO_EXTRACT_BIN";
/// Environment variable selecting the record-once extractor cache directory.
pub const ENV_EXTRACT_CACHE: &str = "TEXO_EXTRACT_CACHE";
/// Environment variable holding the hosted model key.
pub const ENV_MODEL_API_KEY: &str = "TEXO_LLM_API_KEY";
/// Default extraction cache directory relative to the workspace root.
pub const DEFAULT_EXTRACT_CACHE: &str = ".texo/extract-cache";
/// Whether first-run bootstrap may point `extractor_cmd` at `texo extract`.
pub const EXTRACT_SUBCOMMAND_READY: bool = true;

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

/// Resolve bootstrap extraction settings without reading process environment.
#[must_use]
pub fn resolve_bootstrap(root: &Path, inputs: &BootstrapInputs) -> BootstrapDecision {
    if let Some(raw) = &inputs.extract_bin {
        if raw.trim().is_empty() {
            return BootstrapDecision {
                extractor_cmd: None,
                semantics_enabled: false,
                warning: None,
            };
        }
        let cmd = extractor_cmd_for(root, raw);
        return BootstrapDecision {
            extractor_cmd: Some(cmd),
            semantics_enabled: true,
            warning: None,
        };
    }
    let has_key = inputs
        .model_api_key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty());
    if has_key && EXTRACT_SUBCOMMAND_READY {
        let exe = inputs.current_exe.to_string_lossy();
        return BootstrapDecision {
            extractor_cmd: Some(extractor_cmd_for(root, &exe)),
            semantics_enabled: true,
            warning: None,
        };
    }
    BootstrapDecision {
        extractor_cmd: None,
        semantics_enabled: false,
        warning: (!has_key)
            .then(|| "TEXO_LLM_API_KEY is not set; using heuristic session extraction".to_string()),
    }
}

/// Resolve bootstrap extraction settings from process environment.
///
/// # Errors
///
/// Returns [`TexoError::Io`] when the current executable path cannot be read.
pub fn resolve_bootstrap_from_env(root: &Path) -> Result<BootstrapDecision, TexoError> {
    let inputs = BootstrapInputs {
        extract_bin: std::env::var(ENV_EXTRACT_BIN).ok(),
        model_api_key: std::env::var(ENV_MODEL_API_KEY).ok(),
        current_exe: std::env::current_exe()?,
    };
    Ok(resolve_bootstrap(root, &inputs))
}

/// Build the extractor command string stored in config.
#[must_use]
pub fn extractor_cmd_for(root: &Path, exe: &str) -> String {
    let cache = root.join(DEFAULT_EXTRACT_CACHE);
    format!(
        "{ENV_EXTRACT_CACHE}=\"${{{ENV_EXTRACT_CACHE}:-{}}}\" {exe} extract",
        cache.display()
    )
}

/// Ensure the memory workspace config exists.
///
/// # Errors
///
/// Returns [`TexoError::Config`] when config serialization fails and
/// [`TexoError::Io`] when directories or files cannot be written.
pub fn ensure_workspace(
    root: &Path,
    workspace_id: &str,
    decision: &BootstrapDecision,
) -> Result<(), TexoError> {
    let config_path = root.join(".texo").join("config.toml");
    if config_path.exists() {
        return Ok(());
    }
    let mut workspaces = BTreeMap::new();
    workspaces.insert(
        workspace_id.to_string(),
        WorkspaceEntry {
            store_path: format!(".texo/stores/{workspace_id}"),
            docs_glob: format!("{}/**/*.md", crate::ops::agent::SESSIONS_DIR),
            extractor_cmd: decision.extractor_cmd.clone(),
            semantics: decision.semantics_enabled.then(|| SemanticsConfig {
                enabled: true,
                ..SemanticsConfig::default()
            }),
        },
    );
    let config = TexoRootConfig {
        default_workspace: workspace_id.to_string(),
        workspaces,
        gateway: None,
    };
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = toml::to_string_pretty(&config).map_err(|error| TexoError::Config {
        detail: error.to_string(),
        source: Some(Box::new(error)),
    })?;
    std::fs::write(&config_path, raw)?;
    std::fs::create_dir_all(root.join(".texo").join("stores").join(workspace_id))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(bin: Option<&str>, key: Option<&str>) -> BootstrapInputs {
        BootstrapInputs {
            extract_bin: bin.map(str::to_string),
            model_api_key: key.map(str::to_string),
            current_exe: PathBuf::from("/opt/texo"),
        }
    }

    #[test]
    fn explicit_extract_bin_wins() {
        let decision = resolve_bootstrap(Path::new("/root"), &inputs(Some("/bin/extract"), None));
        assert_eq!(
            decision.extractor_cmd.as_deref(),
            Some("TEXO_EXTRACT_CACHE=\"${TEXO_EXTRACT_CACHE:-/root/.texo/extract-cache}\" /bin/extract extract")
        );
        assert!(decision.semantics_enabled);
    }

    #[test]
    fn empty_extract_bin_opts_out_to_heuristic() {
        let decision = resolve_bootstrap(Path::new("/root"), &inputs(Some(""), Some("key")));
        assert_eq!(decision.extractor_cmd, None);
        assert!(!decision.semantics_enabled);
        assert_eq!(decision.warning, None);
    }

    #[test]
    fn unset_with_key_uses_extract_subcommand_when_ready() {
        let decision = resolve_bootstrap(Path::new("/root"), &inputs(None, Some("key")));
        assert_eq!(
            decision.extractor_cmd.as_deref(),
            Some("TEXO_EXTRACT_CACHE=\"${TEXO_EXTRACT_CACHE:-/root/.texo/extract-cache}\" /opt/texo extract")
        );
        assert!(decision.semantics_enabled);
        assert_eq!(decision.warning, None);
    }

    #[test]
    fn unset_without_key_warns_and_uses_heuristic() {
        let decision = resolve_bootstrap(Path::new("/root"), &inputs(None, None));
        assert_eq!(decision.extractor_cmd, None);
        assert!(!decision.semantics_enabled);
        assert_eq!(
            decision.warning.as_deref(),
            Some("TEXO_LLM_API_KEY is not set; using heuristic session extraction")
        );
    }
}
