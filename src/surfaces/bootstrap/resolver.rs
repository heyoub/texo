//! Pure bootstrap resolution policy.

use std::path::Path;

use super::{extractor_cmd_for, BootstrapDecision, BootstrapInputs, EXTRACT_SUBCOMMAND_READY};

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
