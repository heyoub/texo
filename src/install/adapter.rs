//! Client adapter merge, removal, and managed-manifest policy.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::TexoError;

use super::filesystem::{
    append_block, atomic_write, classify_bytes, ensure_safe_managed_path, escape_toml,
    read_optional_string, remove_marked_block, strip_marked_block, with_newline,
};
use super::{
    config_error, ChangeAction, ClientTarget, InstallChange, AGENT_GUIDE_PATH, MCP_MANIFEST_PATH,
};

pub(super) const CLAUDE_MCP_PATH: &str = ".mcp.json";
pub(super) const CURSOR_MCP_PATH: &str = ".cursor/mcp.json";
pub(super) const CODEX_CONFIG_PATH: &str = ".codex/config.toml";
const CODEX_MARKER_START: &str = "# texo:install:codex:start";
const CODEX_MARKER_END: &str = "# texo:install:codex:end";
pub(super) const AGENT_MARKER_START: &str = "<!-- texo:install:start -->";
pub(super) const AGENT_MARKER_END: &str = "<!-- texo:install:end -->";

type OrderedJsonObject = IndexMap<String, Box<serde_json::value::RawValue>>;

pub(super) fn merge_client_adapter(
    root: &Path,
    workspace_id: &str,
    client: ClientTarget,
    journal_id: Option<&str>,
    previous_managed: Option<&Value>,
    dry_run: bool,
) -> Result<Option<InstallChange>, TexoError> {
    let change = match client {
        ClientTarget::Claude => merge_json_adapter(
            root,
            CLAUDE_MCP_PATH,
            workspace_id,
            journal_id,
            previous_managed,
            dry_run,
        )?,
        ClientTarget::Cursor => merge_json_adapter(
            root,
            CURSOR_MCP_PATH,
            workspace_id,
            journal_id,
            previous_managed,
            dry_run,
        )?,
        ClientTarget::Codex => merge_codex_adapter(root, workspace_id, journal_id, dry_run)?,
        ClientTarget::Auto | ClientTarget::All => return Ok(None),
    };
    Ok(Some(change))
}

pub(super) fn managed_server_entry(root: &Path) -> Result<Option<Value>, TexoError> {
    let path = root.join(MCP_MANIFEST_PATH);
    if !path.exists() {
        return Ok(None);
    }
    let manifest = serde_json::from_slice::<Value>(&std::fs::read(path)?)?;
    if manifest.get("schema").and_then(Value::as_str) != Some("texo.mcp-install.v1") {
        return Err(config_error(
            MCP_MANIFEST_PATH,
            "existing manifest is not owned by this installer",
        ));
    }
    Ok(manifest.get("server").cloned())
}

pub(super) fn resolve_clients(root: &Path, requested: &[ClientTarget]) -> Vec<ClientTarget> {
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

pub(super) fn canonical_manifest(
    workspace_id: &str,
    journal_id: Option<&str>,
    created_paths: &BTreeSet<String>,
) -> Value {
    json!({
        "schema": "texo.mcp-install.v1",
        "server": server_entry(workspace_id, journal_id),
        "created_paths": created_paths
    })
}

fn server_entry(workspace_id: &str, journal_id: Option<&str>) -> Value {
    let mut args = vec![
        "--root".to_string(),
        ".".to_string(),
        "--workspace".to_string(),
        workspace_id.to_string(),
    ];
    if let Some(journal_id) = journal_id {
        args.push("--journal".to_string());
        args.push(journal_id.to_string());
    }
    args.push("mcp".to_string());
    json!({
        "command": "texo",
        "args": args,
        "env": {}
    })
}

fn merge_json_adapter(
    root: &Path,
    relative: &str,
    workspace_id: &str,
    journal_id: Option<&str>,
    previous_managed: Option<&Value>,
    dry_run: bool,
) -> Result<InstallChange, TexoError> {
    ensure_safe_managed_path(root, relative)?;
    let path = root.join(relative);
    let existed = path.exists();
    let (mut document, mut servers) = read_ordered_adapter(&path, existed, relative)?;
    let wanted = server_entry(workspace_id, journal_id);
    if let Some(existing) = servers.get("texo") {
        let existing = serde_json::from_str::<Value>(existing.get())?;
        if existing != wanted && previous_managed != Some(&existing) {
            return Err(config_error(
                relative,
                "existing mcpServers.texo is not managed by this installer",
            ));
        }
    }
    servers.insert("texo".to_string(), raw_json(&wanted)?);
    document.insert("mcpServers".to_string(), raw_json(&servers)?);
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
    journal_id: Option<&str>,
    dry_run: bool,
) -> Result<InstallChange, TexoError> {
    ensure_safe_managed_path(root, CODEX_CONFIG_PATH)?;
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
    let journal_args = journal_id.map_or_else(String::new, |journal_id| {
        format!(", \"--journal\", \"{}\"", escape_toml(journal_id))
    });
    let args = format!(
        "[\"--root\", \".\", \"--workspace\", \"{}\"{journal_args}, \"mcp\"]",
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

pub(super) fn upsert_agent_guide(
    root: &Path,
    workspace_id: &str,
    dry_run: bool,
) -> Result<InstallChange, TexoError> {
    ensure_safe_managed_path(root, AGENT_GUIDE_PATH)?;
    let path = root.join(AGENT_GUIDE_PATH);
    let existing = read_optional_string(&path)?;
    let (without, _) = strip_marked_block(&existing, AGENT_MARKER_START, AGENT_MARKER_END)?;
    let block = format!(
        "{AGENT_MARKER_START}\n## Texo agent context\n\nWorkspace: `{workspace_id}`. Start with the `get_agent_context` MCP tool before answering from project knowledge. Reuse its snapshot token with `search_knowledge`, `explain_knowledge`, and `triangulate` so one investigation stays on one frontier. Inspect coverage before treating absence as evidence. Absence of a relation verdict never means unrelated. Texo MCP tools are local and read-only.\n{AGENT_MARKER_END}"
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

pub(super) fn client_path(client: ClientTarget) -> Option<&'static str> {
    match client {
        ClientTarget::Codex => Some(CODEX_CONFIG_PATH),
        ClientTarget::Claude => Some(CLAUDE_MCP_PATH),
        ClientTarget::Cursor => Some(CURSOR_MCP_PATH),
        ClientTarget::Auto | ClientTarget::All => None,
    }
}

fn managed_manifest(root: &Path) -> Result<Option<Value>, TexoError> {
    ensure_safe_managed_path(root, MCP_MANIFEST_PATH)?;
    let path = root.join(MCP_MANIFEST_PATH);
    if !path.exists() {
        return Ok(None);
    }
    let document = serde_json::from_slice::<Value>(&std::fs::read(path)?)?;
    if document.get("schema").and_then(Value::as_str) != Some("texo.mcp-install.v1") {
        return Err(config_error(
            MCP_MANIFEST_PATH,
            "file is not managed by this installer",
        ));
    }
    Ok(Some(document))
}

pub(super) fn managed_created_paths(root: &Path) -> Result<BTreeSet<String>, TexoError> {
    let Some(document) = managed_manifest(root)? else {
        return Ok(BTreeSet::new());
    };
    document.get("created_paths").map_or_else(
        || Ok(BTreeSet::new()),
        |paths| serde_json::from_value(paths.clone()).map_err(TexoError::Json),
    )
}

pub(super) fn managed_workspace_id(root: &Path) -> Result<Option<String>, TexoError> {
    let Some(document) = managed_manifest(root)? else {
        return Ok(None);
    };
    // Recover the workspace from the `--workspace <id>` flag pair, not a fixed
    // positional index. A managed manifest with a valid schema but an
    // unexpected args shape must fail closed rather than silently rewrite the
    // manifest to point future MCP clients at the `demo` workspace.
    let args = document
        .get("server")
        .and_then(|server| server.get("args"))
        .and_then(Value::as_array)
        .ok_or_else(|| config_error(MCP_MANIFEST_PATH, "managed manifest has no server args"))?;
    let workspace = args.windows(2).find_map(|pair| {
        if pair[0].as_str() == Some("--workspace") {
            pair[1].as_str()
        } else {
            None
        }
    });
    match workspace {
        Some(id) => Ok(Some(id.to_string())),
        None => Err(config_error(
            MCP_MANIFEST_PATH,
            "managed manifest args carry no recoverable --workspace id; refusing to rewrite",
        )),
    }
}

pub(super) fn managed_journal_id(root: &Path) -> Result<Option<String>, TexoError> {
    let Some(document) = managed_manifest(root)? else {
        return Ok(None);
    };
    let args = document
        .get("server")
        .and_then(|server| server.get("args"))
        .and_then(Value::as_array)
        .ok_or_else(|| config_error(MCP_MANIFEST_PATH, "managed manifest has no server args"))?;
    Ok(args.windows(2).find_map(|pair| {
        if pair[0].as_str() == Some("--journal") {
            pair[1].as_str().map(str::to_string)
        } else {
            None
        }
    }))
}

pub(super) fn preflight_remove_client(
    root: &Path,
    client: ClientTarget,
    created_paths: &BTreeSet<String>,
) -> Result<(), TexoError> {
    let _change = remove_client(root, client, created_paths, true)?;
    Ok(())
}

pub(super) fn remove_client(
    root: &Path,
    client: ClientTarget,
    created_paths: &BTreeSet<String>,
    dry_run: bool,
) -> Result<Option<InstallChange>, TexoError> {
    let Some(relative) = client_path(client) else {
        return Ok(None);
    };
    let remove_empty = created_paths.contains(relative);
    match client {
        ClientTarget::Claude | ClientTarget::Cursor => {
            remove_json_adapter(root, relative, remove_empty, dry_run)
        }
        ClientTarget::Codex => remove_marked_block(
            root,
            relative,
            CODEX_MARKER_START,
            CODEX_MARKER_END,
            remove_empty,
            dry_run,
        ),
        ClientTarget::Auto | ClientTarget::All => Ok(None),
    }
}

fn remove_json_adapter(
    root: &Path,
    relative: &str,
    remove_empty: bool,
    dry_run: bool,
) -> Result<Option<InstallChange>, TexoError> {
    ensure_safe_managed_path(root, relative)?;
    let path = root.join(relative);
    if !path.exists() {
        return Ok(None);
    }
    let (mut document, mut servers) = read_ordered_adapter(&path, true, relative)?;
    let Some(existing) = servers.get("texo") else {
        return Ok(None);
    };
    if !is_managed_server_entry(&serde_json::from_str::<Value>(existing.get())?) {
        return Err(config_error(
            relative,
            "mcpServers.texo is not managed by this installer",
        ));
    }
    servers.shift_remove("texo");
    document.insert("mcpServers".to_string(), raw_json(&servers)?);
    if !dry_run {
        if remove_empty && json_adapter_is_empty(&document, &servers) {
            std::fs::remove_file(&path)?;
        } else {
            atomic_write(&path, &with_newline(serde_json::to_vec_pretty(&document)?))?;
        }
    }
    Ok(Some(InstallChange {
        path: relative.to_string(),
        action: ChangeAction::Removed,
    }))
}

fn read_ordered_adapter(
    path: &Path,
    existed: bool,
    relative: &str,
) -> Result<(OrderedJsonObject, OrderedJsonObject), TexoError> {
    let document = if existed {
        serde_json::from_slice::<OrderedJsonObject>(&std::fs::read(path)?)?
    } else {
        OrderedJsonObject::new()
    };
    let servers = document.get("mcpServers").map_or_else(
        || Ok(OrderedJsonObject::new()),
        |raw| {
            serde_json::from_str::<OrderedJsonObject>(raw.get()).map_err(|error| {
                TexoError::Config {
                    detail: format!("{relative}: mcpServers must be an object"),
                    source: Some(Box::new(error)),
                }
            })
        },
    )?;
    Ok((document, servers))
}

fn raw_json<T: Serialize>(value: &T) -> Result<Box<serde_json::value::RawValue>, TexoError> {
    serde_json::value::RawValue::from_string(serde_json::to_string(value)?).map_err(TexoError::Json)
}

fn json_adapter_is_empty(document: &OrderedJsonObject, servers: &OrderedJsonObject) -> bool {
    document.len() == 1 && servers.is_empty()
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
                    && args.last().and_then(Value::as_str) == Some("mcp")
                    && (args.len() == 5
                        || (args.len() == 7
                            && args.get(4).and_then(Value::as_str) == Some("--journal")
                            && args.get(5).and_then(Value::as_str).is_some()))
            })
}

pub(super) fn paths() -> [PathBuf; 4] {
    [
        PathBuf::from(MCP_MANIFEST_PATH),
        PathBuf::from(CLAUDE_MCP_PATH),
        PathBuf::from(CURSOR_MCP_PATH),
        PathBuf::from(CODEX_CONFIG_PATH),
    ]
}
