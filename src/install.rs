//! Idempotent workspace appliance installation.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::TexoError;

/// Canonical client-neutral MCP manifest.
pub const MCP_MANIFEST_PATH: &str = ".texo/mcp.json";
/// Root agent guidance file updated through one managed marker block.
pub const AGENT_GUIDE_PATH: &str = "AGENTS.md";

const CLAUDE_MCP_PATH: &str = ".mcp.json";
const CURSOR_MCP_PATH: &str = ".cursor/mcp.json";
const CODEX_CONFIG_PATH: &str = ".codex/config.toml";
const CODEX_MARKER_START: &str = "# texo:install:codex:start";
const CODEX_MARKER_END: &str = "# texo:install:codex:end";
const AGENT_MARKER_START: &str = "<!-- texo:install:start -->";
const AGENT_MARKER_END: &str = "<!-- texo:install:end -->";

/// Agent client adapter target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ClientTarget {
    /// Detect existing client project configuration.
    Auto,
    /// Install a Codex project adapter.
    Codex,
    /// Install a Claude Code project adapter.
    Claude,
    /// Install a Cursor project adapter.
    Cursor,
    /// Install every supported project adapter.
    All,
}

/// Change classification for one managed path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeAction {
    /// File or managed entry would be created.
    Created,
    /// Existing managed bytes would change.
    Updated,
    /// Existing managed bytes already match.
    Unchanged,
    /// Managed content would be removed.
    Removed,
}

/// One install/uninstall path result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallChange {
    /// Workspace-relative path.
    pub path: String,
    /// Action applied or planned.
    pub action: ChangeAction,
}

/// Machine-readable appliance installation report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallReport {
    /// Report schema.
    pub schema: &'static str,
    /// Workspace root.
    pub root: String,
    /// Workspace id installed.
    pub workspace_id: String,
    /// Whether this was a write-free preview.
    pub dry_run: bool,
    /// Selected concrete clients.
    pub clients: Vec<ClientTarget>,
    /// Ordered path changes.
    pub changes: Vec<InstallChange>,
}

/// Machine-readable uninstall report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UninstallReport {
    /// Report schema.
    pub schema: &'static str,
    /// Workspace root.
    pub root: String,
    /// Whether this was a write-free preview.
    pub dry_run: bool,
    /// Ordered path changes.
    pub changes: Vec<InstallChange>,
}

/// Install the lightweight Texo appliance.
///
/// # Errors
/// Returns an error when existing client configuration is malformed, already
/// owns a conflicting `texo` entry, or a managed write fails.
pub fn install(
    root: &Path,
    workspace_id: &str,
    requested: &[ClientTarget],
    dry_run: bool,
) -> Result<InstallReport, TexoError> {
    let clients = resolve_clients(root, requested);
    // Validate every merge before the first write so a conflicting adapter
    // cannot leave behind a half-installed workspace.
    for client in &clients {
        match client {
            ClientTarget::Claude => {
                merge_json_adapter(root, CLAUDE_MCP_PATH, workspace_id, true)?;
            }
            ClientTarget::Cursor => {
                merge_json_adapter(root, CURSOR_MCP_PATH, workspace_id, true)?;
            }
            ClientTarget::Codex => {
                merge_codex_adapter(root, workspace_id, true)?;
            }
            ClientTarget::Auto | ClientTarget::All => {}
        }
    }
    upsert_agent_guide(root, workspace_id, true)?;

    let mut changes = Vec::new();
    let config_path = root.join(".texo/config.toml");
    let config_existed = config_path.exists();
    if !dry_run {
        let decision = crate::surfaces::bootstrap::resolve_bootstrap_from_env(root)?;
        crate::surfaces::bootstrap::ensure_workspace(root, workspace_id, &decision)?;
    }
    changes.push(InstallChange {
        path: ".texo/config.toml".to_string(),
        action: if config_existed {
            ChangeAction::Unchanged
        } else {
            ChangeAction::Created
        },
    });

    let canonical = serde_json::to_vec_pretty(&canonical_manifest(workspace_id))?;
    changes.push(write_managed(
        root,
        MCP_MANIFEST_PATH,
        &with_newline(canonical),
        dry_run,
    )?);
    let hooks = serde_json::to_vec_pretty(&crate::hooks::manifest(workspace_id))?;
    changes.push(write_managed(
        root,
        crate::hooks::HOOKS_MANIFEST_PATH,
        &with_newline(hooks),
        dry_run,
    )?);

    for client in &clients {
        let change = match client {
            ClientTarget::Claude => {
                merge_json_adapter(root, CLAUDE_MCP_PATH, workspace_id, dry_run)
            }
            ClientTarget::Cursor => {
                merge_json_adapter(root, CURSOR_MCP_PATH, workspace_id, dry_run)
            }
            ClientTarget::Codex => merge_codex_adapter(root, workspace_id, dry_run),
            ClientTarget::Auto | ClientTarget::All => continue,
        }?;
        changes.push(change);
    }
    changes.push(upsert_agent_guide(root, workspace_id, dry_run)?);
    Ok(InstallReport {
        schema: "texo.install.v1",
        root: root.display().to_string(),
        workspace_id: workspace_id.to_string(),
        dry_run,
        clients,
        changes,
    })
}

/// Remove only Texo-managed appliance entries, never journals or config.
///
/// # Errors
/// Returns an error when a managed file cannot be parsed or updated.
pub fn uninstall(root: &Path, dry_run: bool) -> Result<UninstallReport, TexoError> {
    let mut changes = Vec::new();
    changes.push(remove_managed_file(
        root,
        MCP_MANIFEST_PATH,
        "texo.mcp-install.v1",
        dry_run,
    )?);
    changes.push(remove_managed_file(
        root,
        crate::hooks::HOOKS_MANIFEST_PATH,
        "texo.hooks.v1",
        dry_run,
    )?);
    for path in [CLAUDE_MCP_PATH, CURSOR_MCP_PATH] {
        if let Some(change) = remove_json_adapter(root, path, dry_run)? {
            changes.push(change);
        }
    }
    if let Some(change) = remove_marked_block(
        root,
        CODEX_CONFIG_PATH,
        CODEX_MARKER_START,
        CODEX_MARKER_END,
        dry_run,
    )? {
        changes.push(change);
    }
    if let Some(change) = remove_marked_block(
        root,
        AGENT_GUIDE_PATH,
        AGENT_MARKER_START,
        AGENT_MARKER_END,
        dry_run,
    )? {
        changes.push(change);
    }
    Ok(UninstallReport {
        schema: "texo.uninstall.v1",
        root: root.display().to_string(),
        dry_run,
        changes,
    })
}

fn resolve_clients(root: &Path, requested: &[ClientTarget]) -> Vec<ClientTarget> {
    let requested = if requested.is_empty() {
        &[ClientTarget::Auto][..]
    } else {
        requested
    };
    let mut selected = BTreeSet::new();
    for target in requested {
        match target {
            ClientTarget::All => {
                selected.extend([
                    ClientTarget::Codex,
                    ClientTarget::Claude,
                    ClientTarget::Cursor,
                ]);
            }
            ClientTarget::Auto => {
                if root.join(".codex").exists() {
                    selected.insert(ClientTarget::Codex);
                }
                if root.join(CLAUDE_MCP_PATH).exists() || root.join(".claude").exists() {
                    selected.insert(ClientTarget::Claude);
                }
                if root.join(".cursor").exists() {
                    selected.insert(ClientTarget::Cursor);
                }
            }
            concrete @ (ClientTarget::Codex | ClientTarget::Claude | ClientTarget::Cursor) => {
                selected.insert(*concrete);
            }
        }
    }
    selected.into_iter().collect()
}

fn canonical_manifest(workspace_id: &str) -> Value {
    json!({
        "schema": "texo.mcp-install.v1",
        "server": server_entry(workspace_id)
    })
}

fn server_entry(workspace_id: &str) -> Value {
    json!({
        "command": "texo",
        "args": ["--root", ".", "--workspace", workspace_id, "mcp"],
        "env": {}
    })
}

fn merge_json_adapter(
    root: &Path,
    relative: &str,
    workspace_id: &str,
    dry_run: bool,
) -> Result<InstallChange, TexoError> {
    let path = root.join(relative);
    let existed = path.exists();
    let mut document = if existed {
        serde_json::from_slice::<Value>(&std::fs::read(&path)?)?
    } else {
        json!({})
    };
    let object = document
        .as_object_mut()
        .ok_or_else(|| config_error(relative, "root must be an object"))?;
    let servers = object
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| config_error(relative, "mcpServers must be an object"))?;
    let wanted = server_entry(workspace_id);
    if let Some(existing) = servers.get("texo") {
        if existing != &wanted {
            return Err(config_error(
                relative,
                "existing mcpServers.texo is not managed by this installer",
            ));
        }
    }
    servers.insert("texo".to_string(), wanted);
    let bytes = with_newline(serde_json::to_vec_pretty(&document)?);
    let action = classify_bytes(&path, &bytes)?;
    if !dry_run && action != ChangeAction::Unchanged {
        atomic_write(&path, &bytes)?;
    }
    Ok(InstallChange {
        path: relative.to_string(),
        action: if !existed && action == ChangeAction::Updated {
            ChangeAction::Created
        } else {
            action
        },
    })
}

fn merge_codex_adapter(
    root: &Path,
    workspace_id: &str,
    dry_run: bool,
) -> Result<InstallChange, TexoError> {
    let path = root.join(CODEX_CONFIG_PATH);
    let existing = read_optional_string(&path)?;
    let (without, had_marker) =
        strip_marked_block(&existing, CODEX_MARKER_START, CODEX_MARKER_END)?;
    if !without.trim().is_empty() {
        let parsed = without
            .parse::<toml::Value>()
            .map_err(|error| TexoError::Config {
                detail: format!("{CODEX_CONFIG_PATH}: {error}"),
                source: Some(Box::new(error)),
            })?;
        if parsed
            .get("mcp_servers")
            .and_then(|value| value.get("texo"))
            .is_some()
        {
            return Err(config_error(
                CODEX_CONFIG_PATH,
                "existing mcp_servers.texo is outside the managed block",
            ));
        }
    }
    let args = format!(
        "[\"--root\", \".\", \"--workspace\", \"{}\", \"mcp\"]",
        escape_toml(workspace_id)
    );
    let block = format!(
        "{CODEX_MARKER_START}\n[mcp_servers.texo]\ncommand = \"texo\"\nargs = {args}\n{CODEX_MARKER_END}"
    );
    let updated = append_block(&without, &block);
    let bytes = updated.into_bytes();
    let action = classify_bytes(&path, &bytes)?;
    if !dry_run && action != ChangeAction::Unchanged {
        atomic_write(&path, &bytes)?;
    }
    Ok(InstallChange {
        path: CODEX_CONFIG_PATH.to_string(),
        action: if !path.exists() && !had_marker {
            ChangeAction::Created
        } else {
            action
        },
    })
}

fn upsert_agent_guide(
    root: &Path,
    workspace_id: &str,
    dry_run: bool,
) -> Result<InstallChange, TexoError> {
    let path = root.join(AGENT_GUIDE_PATH);
    let existing = read_optional_string(&path)?;
    let (without, _) = strip_marked_block(&existing, AGENT_MARKER_START, AGENT_MARKER_END)?;
    let block = format!(
        "{AGENT_MARKER_START}\n## Texo agent context\n\nWorkspace: `{workspace_id}`. Start with the `get_agent_context` MCP tool before answering from project knowledge. Use `search_claims` for bounded discovery, `explain_claim` for provenance, and `check_staleness` before trusting or editing documentation. Absence of a relation verdict never means unrelated. Texo MCP tools are local and read-only.\n{AGENT_MARKER_END}"
    );
    let bytes = append_block(&without, &block).into_bytes();
    let action = classify_bytes(&path, &bytes)?;
    if !dry_run && action != ChangeAction::Unchanged {
        atomic_write(&path, &bytes)?;
    }
    Ok(InstallChange {
        path: AGENT_GUIDE_PATH.to_string(),
        action: if path.exists() {
            action
        } else {
            ChangeAction::Created
        },
    })
}

fn remove_json_adapter(
    root: &Path,
    relative: &str,
    dry_run: bool,
) -> Result<Option<InstallChange>, TexoError> {
    let path = root.join(relative);
    if !path.exists() {
        return Ok(None);
    }
    let mut document = serde_json::from_slice::<Value>(&std::fs::read(&path)?)?;
    let Some(existing) = document
        .get("mcpServers")
        .and_then(Value::as_object)
        .and_then(|servers| servers.get("texo"))
    else {
        return Ok(None);
    };
    if !is_managed_server_entry(existing) {
        return Err(config_error(
            relative,
            "mcpServers.texo is not managed by this installer",
        ));
    }
    document
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .map(|servers| servers.remove("texo"));
    if !dry_run {
        atomic_write(&path, &with_newline(serde_json::to_vec_pretty(&document)?))?;
    }
    Ok(Some(InstallChange {
        path: relative.to_string(),
        action: ChangeAction::Removed,
    }))
}

fn remove_managed_file(
    root: &Path,
    relative: &str,
    schema: &str,
    dry_run: bool,
) -> Result<InstallChange, TexoError> {
    let path = root.join(relative);
    let action = if path.is_file() {
        let document = serde_json::from_slice::<Value>(&std::fs::read(&path)?)?;
        if document.get("schema").and_then(Value::as_str) != Some(schema) {
            return Err(config_error(
                relative,
                "file is not managed by this installer",
            ));
        }
        if !dry_run {
            std::fs::remove_file(path)?;
        }
        ChangeAction::Removed
    } else {
        ChangeAction::Unchanged
    };
    Ok(InstallChange {
        path: relative.to_string(),
        action,
    })
}

fn is_managed_server_entry(value: &Value) -> bool {
    value.get("command").and_then(Value::as_str) == Some("texo")
        && value
            .get("args")
            .and_then(Value::as_array)
            .is_some_and(|args| {
                args.first().and_then(Value::as_str) == Some("--root")
                    && args.get(1).and_then(Value::as_str) == Some(".")
                    && args.get(2).and_then(Value::as_str) == Some("--workspace")
                    && args.get(3).and_then(Value::as_str).is_some()
                    && args.get(4).and_then(Value::as_str) == Some("mcp")
            })
}

fn remove_marked_block(
    root: &Path,
    relative: &str,
    start: &str,
    end: &str,
    dry_run: bool,
) -> Result<Option<InstallChange>, TexoError> {
    let path = root.join(relative);
    let existing = read_optional_string(&path)?;
    let (without, had_marker) = strip_marked_block(&existing, start, end)?;
    if !had_marker {
        return Ok(None);
    }
    if !dry_run {
        atomic_write(&path, without.as_bytes())?;
    }
    Ok(Some(InstallChange {
        path: relative.to_string(),
        action: ChangeAction::Removed,
    }))
}

fn write_managed(
    root: &Path,
    relative: &str,
    bytes: &[u8],
    dry_run: bool,
) -> Result<InstallChange, TexoError> {
    let path = root.join(relative);
    let existed = path.exists();
    let action = classify_bytes(&path, bytes)?;
    if !dry_run && action != ChangeAction::Unchanged {
        atomic_write(&path, bytes)?;
    }
    Ok(InstallChange {
        path: relative.to_string(),
        action: if existed {
            action
        } else {
            ChangeAction::Created
        },
    })
}

fn classify_bytes(path: &Path, wanted: &[u8]) -> Result<ChangeAction, TexoError> {
    match std::fs::read(path) {
        Ok(existing) if existing == wanted => Ok(ChangeAction::Unchanged),
        Ok(_) => Ok(ChangeAction::Updated),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ChangeAction::Created),
        Err(error) => Err(error.into()),
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), TexoError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("texo-install-{}.tmp", std::process::id()));
    let result = (|| {
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _removed = std::fs::remove_file(tmp);
    }
    result.map_err(Into::into)
}

fn strip_marked_block(input: &str, start: &str, end: &str) -> Result<(String, bool), TexoError> {
    let Some(start_offset) = input.find(start) else {
        if input.contains(end) {
            return Err(config_error(
                AGENT_GUIDE_PATH,
                "orphaned managed end marker",
            ));
        }
        return Ok((input.to_string(), false));
    };
    let Some(relative_end) = input[start_offset..].find(end) else {
        return Err(config_error(
            AGENT_GUIDE_PATH,
            "unterminated managed marker block",
        ));
    };
    let end_offset = start_offset + relative_end + end.len();
    if input[end_offset..].contains(start) {
        return Err(config_error(
            AGENT_GUIDE_PATH,
            "multiple managed marker blocks",
        ));
    }
    let mut without = String::new();
    without.push_str(input[..start_offset].trim_end());
    if !without.is_empty() {
        without.push('\n');
    }
    without.push_str(input[end_offset..].trim_start_matches(['\r', '\n']));
    Ok((without, true))
}

fn append_block(existing: &str, block: &str) -> String {
    let mut out = existing.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(block);
    out.push('\n');
    out
}

fn read_optional_string(path: &Path) -> Result<String, TexoError> {
    match std::fs::read_to_string(path) {
        Ok(value) => Ok(value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error.into()),
    }
}

fn with_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.push(b'\n');
    bytes
}

fn escape_toml(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn config_error(path: &str, detail: &str) -> TexoError {
    TexoError::Config {
        detail: format!("{path}: {detail}"),
        source: None,
    }
}

/// Return the managed adapter paths for doctor diagnostics.
#[must_use]
pub fn adapter_paths() -> [PathBuf; 4] {
    [
        PathBuf::from(MCP_MANIFEST_PATH),
        PathBuf::from(CLAUDE_MCP_PATH),
        PathBuf::from(CURSOR_MCP_PATH),
        PathBuf::from(CODEX_CONFIG_PATH),
    ]
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn install_all_is_idempotent_and_uninstall_preserves_user_content() {
        let dir = TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join(AGENT_GUIDE_PATH), "# User guide\n").expect("user guide");
        std::fs::create_dir_all(dir.path().join(".codex")).expect("codex dir");
        std::fs::write(dir.path().join(CODEX_CONFIG_PATH), "model = \"example\"\n")
            .expect("codex config");
        std::fs::write(
            dir.path().join(CLAUDE_MCP_PATH),
            b"{\"user\":true,\"mcpServers\":{}}",
        )
        .expect("claude config");

        install(dir.path(), "demo", &[ClientTarget::All], false).expect("install");
        let first = fingerprint_files(dir.path());
        let second = install(dir.path(), "demo", &[ClientTarget::All], false).expect("reinstall");
        assert!(second
            .changes
            .iter()
            .all(|change| change.action == ChangeAction::Unchanged));
        assert_eq!(fingerprint_files(dir.path()), first);

        uninstall(dir.path(), false).expect("uninstall");
        assert!(std::fs::read_to_string(dir.path().join(AGENT_GUIDE_PATH))
            .expect("guide")
            .contains("# User guide"));
        assert!(std::fs::read_to_string(dir.path().join(CODEX_CONFIG_PATH))
            .expect("codex")
            .contains("model = \"example\""));
        let claude: Value = serde_json::from_slice(
            &std::fs::read(dir.path().join(CLAUDE_MCP_PATH)).expect("claude"),
        )
        .expect("valid json");
        assert_eq!(claude["user"], true);
        assert!(claude["mcpServers"].get("texo").is_none());
        assert!(dir.path().join(".texo/config.toml").is_file());
    }

    #[test]
    fn dry_run_is_write_free_on_a_fresh_root() {
        let dir = TempDir::new().expect("tempdir");
        let report = install(dir.path(), "demo", &[ClientTarget::All], true).expect("preview");
        assert!(report.dry_run);
        assert!(report
            .changes
            .iter()
            .all(|change| change.action == ChangeAction::Created));
        assert!(fingerprint_files(dir.path()).is_empty());
    }

    #[test]
    fn conflicting_adapter_fails_before_any_write() {
        let dir = TempDir::new().expect("tempdir");
        let conflict = br#"{"mcpServers":{"texo":{"command":"other"}}}"#;
        std::fs::write(dir.path().join(CLAUDE_MCP_PATH), conflict).expect("conflict");
        let before = fingerprint_files(dir.path());

        let error = install(dir.path(), "demo", &[ClientTarget::All], false)
            .expect_err("conflict must fail");

        assert!(error.to_string().contains("not managed"));
        assert_eq!(fingerprint_files(dir.path()), before);
    }

    #[test]
    fn uninstall_refuses_an_unmanaged_texo_entry() {
        let dir = TempDir::new().expect("tempdir");
        let conflict = br#"{"mcpServers":{"texo":{"command":"other"}}}"#;
        std::fs::write(dir.path().join(CLAUDE_MCP_PATH), conflict).expect("conflict");

        let error = uninstall(dir.path(), false).expect_err("unmanaged entry must survive");

        assert!(error.to_string().contains("not managed"));
        assert_eq!(
            std::fs::read(dir.path().join(CLAUDE_MCP_PATH)).expect("preserved"),
            conflict
        );
    }

    fn fingerprint_files(root: &Path) -> Vec<(String, Vec<u8>)> {
        let mut rows = walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| {
                (
                    entry
                        .path()
                        .strip_prefix(root)
                        .expect("relative")
                        .to_string_lossy()
                        .to_string(),
                    std::fs::read(entry.path()).expect("read"),
                )
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| left.0.cmp(&right.0));
        rows
    }
}
