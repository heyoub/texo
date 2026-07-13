//! Fixed, advisory agent-hook contracts.

use std::path::{Component, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::TexoError;

/// Managed advisory-hook manifest.
pub const HOOKS_MANIFEST_PATH: &str = ".texo/hooks.json";
/// Maximum accepted files-changed input size.
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
/// Maximum paths checked by one files-changed hook.
pub const MAX_CHANGED_PATHS: usize = 64;

/// Bounded files-changed input envelope.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FilesChangedInput {
    /// Workspace-relative paths to check.
    pub paths: Vec<PathBuf>,
}

/// Build the client-neutral hook manifest installed in `.texo`.
#[must_use]
pub fn manifest(workspace_id: &str) -> Value {
    manifest_for_journal(workspace_id, None)
}

/// Build the hook manifest pinned to an optional physical journal.
#[must_use]
pub fn manifest_for_journal(workspace_id: &str, journal_id: Option<&str>) -> Value {
    json!({
        "schema": "texo.hooks.v1",
        "hooks": [
            hook("session_start", workspace_id, journal_id, "session-start", "none"),
            hook("files_changed", workspace_id, journal_id, "files-changed", "texo.hook.files-changed.input.v1"),
            hook("pre_commit", workspace_id, journal_id, "pre-commit", "none")
        ]
    })
}

/// Parse and validate a files-changed envelope.
///
/// # Errors
/// Returns an input error for an oversized body, too many paths, absolute
/// paths, or paths that escape the workspace lexically.
pub fn parse_files_changed(bytes: &[u8]) -> Result<FilesChangedInput, TexoError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(input_error(format!(
            "input exceeds the {MAX_INPUT_BYTES}-byte limit"
        )));
    }
    let input: FilesChangedInput = serde_json::from_slice(bytes)?;
    if input.paths.len() > MAX_CHANGED_PATHS {
        return Err(input_error(format!(
            "paths exceeds the {MAX_CHANGED_PATHS}-item limit"
        )));
    }
    for path in &input.paths {
        if path.as_os_str().is_empty()
            || path.is_absolute()
            || path.components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(input_error(format!(
                "path `{}` must be workspace-relative and may not escape the root",
                path.display()
            )));
        }
    }
    Ok(input)
}

fn hook(
    event: &str,
    workspace_id: &str,
    journal_id: Option<&str>,
    command: &str,
    input_schema: &str,
) -> Value {
    let mut invocation = vec![
        "texo".to_string(),
        "--root".to_string(),
        ".".to_string(),
        "--workspace".to_string(),
        workspace_id.to_string(),
    ];
    if let Some(journal_id) = journal_id {
        invocation.push("--journal".to_string());
        invocation.push(journal_id.to_string());
    }
    invocation.extend([
        "hook".to_string(),
        command.to_string(),
        "--json".to_string(),
    ]);
    json!({
        "event": event,
        "blocking": false,
        "command": invocation,
        "input_schema": input_schema,
        "effect": "read",
        "failure_policy": "warn"
    })
}

fn input_error(detail: String) -> TexoError {
    TexoError::OpInput {
        op: "texo hook files-changed".to_string(),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_paths_are_bounded_and_workspace_relative() {
        let parsed =
            parse_files_changed(br#"{"paths":["docs/a.md","README.md"]}"#).expect("valid paths");
        assert_eq!(parsed.paths.len(), 2);
        assert!(parse_files_changed(br#"{"paths":["../secret"]}"#).is_err());
        assert!(parse_files_changed(br#"{"paths":["/etc/passwd"]}"#).is_err());
        assert!(parse_files_changed(&vec![b' '; MAX_INPUT_BYTES + 1]).is_err());
    }

    #[test]
    fn manifest_contains_only_fixed_advisory_read_hooks() {
        let value = manifest("demo");
        let hooks = value["hooks"].as_array().expect("hooks");
        assert_eq!(hooks.len(), 3);
        assert!(hooks.iter().all(|hook| {
            hook["blocking"] == false
                && hook["effect"] == "read"
                && hook["failure_policy"] == "warn"
                && hook["command"][0] == "texo"
        }));
    }
}
