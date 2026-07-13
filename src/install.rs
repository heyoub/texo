//! Idempotent workspace appliance installation.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use clap::ValueEnum;
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::TexoError;
use crate::topology::{JournalEntry, JournalRole, ReplicaMode};

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
static INSTALL_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

type OrderedJsonObject = IndexMap<String, Box<serde_json::value::RawValue>>;

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
    /// Physical read journal selected for each client adapter.
    pub routes: Vec<ClientJournalRoute>,
    /// Ordered path changes.
    pub changes: Vec<InstallChange>,
}

/// One agent adapter's scale-out journal route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientJournalRoute {
    /// Concrete agent client.
    pub client: ClientTarget,
    /// Physical journal passed to the MCP reader.
    pub journal_id: String,
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
    /// Selected concrete clients.
    pub clients: Vec<ClientTarget>,
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
    install_for_journal(root, workspace_id, None, requested, dry_run)
}

/// Install the appliance with every client pinned to one physical journal.
///
/// # Errors
/// Returns the same safe-merge failures as [`install`].
pub fn install_for_journal(
    root: &Path,
    workspace_id: &str,
    journal_id: Option<&str>,
    requested: &[ClientTarget],
    dry_run: bool,
) -> Result<InstallReport, TexoError> {
    let clients = resolve_clients(root, requested);
    let created_paths = prepare_install_paths(root, &clients)?;
    let config_path = root.join(".texo/config.toml");
    let config_existed = config_path.exists();
    let decision = crate::surfaces::bootstrap::resolve_bootstrap_from_env(root)?;
    let mut root_config = if config_existed {
        crate::config::TexoRootConfig::load(&config_path).map_err(|error| TexoError::Config {
            detail: error.to_string(),
            source: Some(Box::new(error)),
        })?
    } else {
        crate::surfaces::bootstrap::prospective_config(workspace_id, &decision)
    };
    let (routes, topology_changed) =
        plan_client_routes(&mut root_config, workspace_id, journal_id, &clients)?;
    let previous_server = managed_server_entry(root)?;
    // Validate every merge before the first write so a conflicting adapter
    // cannot leave behind a half-installed workspace.
    for client in &clients {
        let _preflight = merge_client_adapter(
            root,
            workspace_id,
            *client,
            route_for(&routes, *client),
            previous_server.as_ref(),
            true,
        )?;
    }
    upsert_agent_guide(root, workspace_id, true)?;

    let mut changes = Vec::new();
    if !dry_run {
        crate::surfaces::bootstrap::ensure_workspace(root, workspace_id, &decision)?;
        if topology_changed {
            root_config
                .save(&config_path)
                .map_err(|error| TexoError::Config {
                    detail: error.to_string(),
                    source: Some(Box::new(error)),
                })?;
        }
        for route in &routes {
            let _report =
                crate::replication::refresh_reader(root, Some(workspace_id), &route.journal_id)?;
        }
    }
    changes.push(InstallChange {
        path: ".texo/config.toml".to_string(),
        action: if !config_existed {
            ChangeAction::Created
        } else if topology_changed {
            ChangeAction::Updated
        } else {
            ChangeAction::Unchanged
        },
    });

    let canonical = serde_json::to_vec_pretty(&canonical_manifest(
        workspace_id,
        journal_id,
        &created_paths,
    ))?;
    changes.push(write_managed(
        root,
        MCP_MANIFEST_PATH,
        &with_newline(canonical),
        dry_run,
    )?);
    let hooks = serde_json::to_vec_pretty(&crate::hooks::manifest_for_journal(
        workspace_id,
        journal_id,
    ))?;
    changes.push(write_managed(
        root,
        crate::hooks::HOOKS_MANIFEST_PATH,
        &with_newline(hooks),
        dry_run,
    )?);

    for client in &clients {
        let Some(change) = merge_client_adapter(
            root,
            workspace_id,
            *client,
            route_for(&routes, *client),
            previous_server.as_ref(),
            dry_run,
        )?
        else {
            continue;
        };
        changes.push(change);
    }
    changes.push(upsert_agent_guide(root, workspace_id, dry_run)?);
    Ok(InstallReport {
        schema: "texo.install.v2",
        root: root.display().to_string(),
        workspace_id: workspace_id.to_string(),
        dry_run,
        clients,
        routes,
        changes,
    })
}

fn prepare_install_paths(
    root: &Path,
    clients: &[ClientTarget],
) -> Result<BTreeSet<String>, TexoError> {
    for relative in [
        ".texo/config.toml",
        MCP_MANIFEST_PATH,
        crate::hooks::HOOKS_MANIFEST_PATH,
        AGENT_GUIDE_PATH,
    ] {
        ensure_safe_managed_path(root, relative)?;
    }
    let mut created_paths = managed_created_paths(root)?;
    for client in clients {
        if let Some(relative) = client_path(*client) {
            ensure_safe_managed_path(root, relative)?;
            if !root.join(relative).exists() {
                created_paths.insert(relative.to_string());
            }
        }
    }
    if !root.join(AGENT_GUIDE_PATH).exists() {
        created_paths.insert(AGENT_GUIDE_PATH.to_string());
    }
    Ok(created_paths)
}

fn merge_client_adapter(
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

fn plan_client_routes(
    config: &mut crate::config::TexoRootConfig,
    workspace_id: &str,
    explicit_journal: Option<&str>,
    clients: &[ClientTarget],
) -> Result<(Vec<ClientJournalRoute>, bool), TexoError> {
    if let Some(journal_id) = explicit_journal {
        config
            .resolve_journal(Some(workspace_id), Some(journal_id))
            .map_err(|error| config_error(".texo/config.toml", &error.to_string()))?;
        return Ok((
            clients
                .iter()
                .copied()
                .map(|client| ClientJournalRoute {
                    client,
                    journal_id: journal_id.to_string(),
                })
                .collect(),
            false,
        ));
    }
    let workspace = config.workspaces.get_mut(workspace_id).ok_or_else(|| {
        config_error(
            ".texo/config.toml",
            &format!("unknown workspace `{workspace_id}`"),
        )
    })?;
    let source = workspace.primary_journal.clone();
    let mut changed = false;
    let mut routes = Vec::with_capacity(clients.len());
    for client in clients {
        let journal_id = client_route_id(*client)
            .ok_or_else(|| config_error("install", "client routing requires a concrete client"))?;
        if let Some(existing) = workspace.journals.get(journal_id) {
            let compatible = existing.role == JournalRole::Replica
                && existing.replica_mode == Some(ReplicaMode::ImportedReadModel)
                && existing.source_journal.as_deref() == Some(source.as_str())
                && existing.source_endpoint.is_none();
            if !compatible {
                return Err(config_error(
                    ".texo/config.toml",
                    &format!(
                        "journal `{journal_id}` is reserved for the {journal_id} adapter but is not a local imported replica of `{source}`"
                    ),
                ));
            }
        } else {
            workspace.journals.insert(
                journal_id.to_string(),
                JournalEntry::replica(
                    format!(".texo/replicas/{workspace_id}/{journal_id}"),
                    source.clone(),
                    ReplicaMode::ImportedReadModel,
                ),
            );
            changed = true;
        }
        routes.push(ClientJournalRoute {
            client: *client,
            journal_id: journal_id.to_string(),
        });
    }
    config
        .resolve_journal(Some(workspace_id), None)
        .map_err(|error| config_error(".texo/config.toml", &error.to_string()))?;
    Ok((routes, changed))
}

fn client_route_id(client: ClientTarget) -> Option<&'static str> {
    match client {
        ClientTarget::Claude => Some("claude"),
        ClientTarget::Codex => Some("codex"),
        ClientTarget::Cursor => Some("cursor"),
        ClientTarget::Auto | ClientTarget::All => None,
    }
}

fn route_for(routes: &[ClientJournalRoute], client: ClientTarget) -> Option<&str> {
    routes
        .iter()
        .find(|route| route.client == client)
        .map(|route| route.journal_id.as_str())
}

fn managed_server_entry(root: &Path) -> Result<Option<Value>, TexoError> {
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

/// Remove only Texo-managed appliance entries, never journals or config.
///
/// # Errors
/// Returns an error when a managed file cannot be parsed or updated.
pub fn uninstall(
    root: &Path,
    requested: &[ClientTarget],
    dry_run: bool,
) -> Result<UninstallReport, TexoError> {
    let remove_shared = requested.is_empty() || requested.contains(&ClientTarget::All);
    let clients = if requested.is_empty() {
        vec![
            ClientTarget::Codex,
            ClientTarget::Claude,
            ClientTarget::Cursor,
        ]
    } else {
        resolve_clients(root, requested)
    };
    let created_paths = managed_created_paths(root)?;

    // Prove every selected entry is ours before the first removal.
    for client in &clients {
        preflight_remove_client(root, *client, &created_paths)?;
    }
    if remove_shared {
        remove_marked_block(
            root,
            AGENT_GUIDE_PATH,
            AGENT_MARKER_START,
            AGENT_MARKER_END,
            created_paths.contains(AGENT_GUIDE_PATH),
            true,
        )?;
        remove_managed_file(
            root,
            crate::hooks::HOOKS_MANIFEST_PATH,
            "texo.hooks.v1",
            true,
        )?;
        remove_managed_file(root, MCP_MANIFEST_PATH, "texo.mcp-install.v1", true)?;
    }

    let mut changes = Vec::new();
    for client in &clients {
        if let Some(change) = remove_client(root, *client, &created_paths, dry_run)? {
            changes.push(change);
        }
    }
    if remove_shared {
        if let Some(change) = remove_marked_block(
            root,
            AGENT_GUIDE_PATH,
            AGENT_MARKER_START,
            AGENT_MARKER_END,
            created_paths.contains(AGENT_GUIDE_PATH),
            dry_run,
        )? {
            changes.push(change);
        }
        changes.push(remove_managed_file(
            root,
            crate::hooks::HOOKS_MANIFEST_PATH,
            "texo.hooks.v1",
            dry_run,
        )?);
        changes.push(remove_managed_file(
            root,
            MCP_MANIFEST_PATH,
            "texo.mcp-install.v1",
            dry_run,
        )?);
    } else {
        let mut remaining = created_paths;
        for client in &clients {
            if let Some(relative) = client_path(*client) {
                remaining.remove(relative);
            }
        }
        if root.join(MCP_MANIFEST_PATH).exists() {
            let Some(workspace_id) = managed_workspace_id(root)? else {
                return Err(config_error(
                    MCP_MANIFEST_PATH,
                    "managed manifest exists but its workspace id could not be recovered",
                ));
            };
            let journal_id = managed_journal_id(root)?;
            let canonical = with_newline(serde_json::to_vec_pretty(&canonical_manifest(
                &workspace_id,
                journal_id.as_deref(),
                &remaining,
            ))?);
            changes.push(write_managed(root, MCP_MANIFEST_PATH, &canonical, dry_run)?);
        }
    }
    Ok(UninstallReport {
        schema: "texo.uninstall.v1",
        root: root.display().to_string(),
        dry_run,
        clients,
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

fn canonical_manifest(
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

fn upsert_agent_guide(
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

fn client_path(client: ClientTarget) -> Option<&'static str> {
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

fn managed_created_paths(root: &Path) -> Result<BTreeSet<String>, TexoError> {
    let Some(document) = managed_manifest(root)? else {
        return Ok(BTreeSet::new());
    };
    document.get("created_paths").map_or_else(
        || Ok(BTreeSet::new()),
        |paths| serde_json::from_value(paths.clone()).map_err(TexoError::Json),
    )
}

fn managed_workspace_id(root: &Path) -> Result<Option<String>, TexoError> {
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

fn managed_journal_id(root: &Path) -> Result<Option<String>, TexoError> {
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

fn preflight_remove_client(
    root: &Path,
    client: ClientTarget,
    created_paths: &BTreeSet<String>,
) -> Result<(), TexoError> {
    let _change = remove_client(root, client, created_paths, true)?;
    Ok(())
}

fn remove_client(
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

fn remove_managed_file(
    root: &Path,
    relative: &str,
    schema: &str,
    dry_run: bool,
) -> Result<InstallChange, TexoError> {
    ensure_safe_managed_path(root, relative)?;
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
                    && args.last().and_then(Value::as_str) == Some("mcp")
                    && (args.len() == 5
                        || (args.len() == 7
                            && args.get(4).and_then(Value::as_str) == Some("--journal")
                            && args.get(5).and_then(Value::as_str).is_some()))
            })
}

fn remove_marked_block(
    root: &Path,
    relative: &str,
    start: &str,
    end: &str,
    remove_empty: bool,
    dry_run: bool,
) -> Result<Option<InstallChange>, TexoError> {
    ensure_safe_managed_path(root, relative)?;
    let path = root.join(relative);
    let existing = read_optional_string(&path)?;
    let (without, had_marker) = strip_marked_block(&existing, start, end)?;
    if !had_marker {
        return Ok(None);
    }
    if !dry_run {
        if remove_empty && without.trim().is_empty() {
            std::fs::remove_file(&path)?;
        } else {
            atomic_write(&path, without.as_bytes())?;
        }
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
    ensure_safe_managed_path(root, relative)?;
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

fn ensure_safe_managed_path(root: &Path, relative: &str) -> Result<(), TexoError> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err(config_error(
            relative,
            "managed path must remain below the workspace root",
        ));
    }
    let mut current = root.to_path_buf();
    for component in relative_path.components() {
        if let std::path::Component::Normal(name) = component {
            current.push(name);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(config_error(
                        relative,
                        &format!("managed path crosses symbolic link `{}`", current.display()),
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), TexoError> {
    let parent = path
        .parent()
        .ok_or_else(|| config_error(&path.display().to_string(), "managed path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let existing_permissions = std::fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.file_type().is_file())
        .map(|metadata| metadata.permissions());
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| config_error(&path.display().to_string(), "file name is not UTF-8"))?;
    for _attempt in 0..100 {
        let counter = INSTALL_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(
            ".{name}.texo-install-{}-{counter}.tmp",
            std::process::id()
        ));
        let mut file = match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        let result = (|| -> std::io::Result<()> {
            file.write_all(bytes)?;
            if let Some(permissions) = &existing_permissions {
                file.set_permissions(permissions.clone())?;
            }
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, path)?;
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _removed = std::fs::remove_file(&tmp);
        }
        return result.map_err(Into::into);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a private install staging file",
    )
    .into())
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

        uninstall(dir.path(), &[], false).expect("uninstall");
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

        let error = uninstall(dir.path(), &[], false).expect_err("unmanaged entry must survive");

        assert!(error.to_string().contains("not managed"));
        assert_eq!(
            std::fs::read(dir.path().join(CLAUDE_MCP_PATH)).expect("preserved"),
            conflict
        );
    }

    #[test]
    fn uninstall_deletes_only_empty_files_created_by_texo() {
        let created = TempDir::new().expect("created root");
        install(created.path(), "demo", &[ClientTarget::All], false).expect("install created");
        uninstall(created.path(), &[], false).expect("uninstall created");
        for relative in [
            CLAUDE_MCP_PATH,
            CURSOR_MCP_PATH,
            CODEX_CONFIG_PATH,
            AGENT_GUIDE_PATH,
        ] {
            assert!(
                !created.path().join(relative).exists(),
                "{relative} removed"
            );
        }
        assert!(created.path().join(".texo/config.toml").is_file());

        let existing = TempDir::new().expect("existing root");
        std::fs::create_dir_all(existing.path().join(".cursor")).expect("cursor dir");
        std::fs::create_dir_all(existing.path().join(".codex")).expect("codex dir");
        std::fs::write(existing.path().join(CLAUDE_MCP_PATH), "{}\n").expect("claude");
        std::fs::write(existing.path().join(CURSOR_MCP_PATH), "{}\n").expect("cursor");
        std::fs::write(existing.path().join(CODEX_CONFIG_PATH), "").expect("codex");
        std::fs::write(existing.path().join(AGENT_GUIDE_PATH), "").expect("guide");
        install(existing.path(), "demo", &[ClientTarget::All], false).expect("install existing");
        uninstall(existing.path(), &[], false).expect("uninstall existing");
        for relative in [
            CLAUDE_MCP_PATH,
            CURSOR_MCP_PATH,
            CODEX_CONFIG_PATH,
            AGENT_GUIDE_PATH,
        ] {
            assert!(
                existing.path().join(relative).is_file(),
                "{relative} preserved"
            );
        }
    }

    #[test]
    fn targeted_uninstall_keeps_shared_and_other_client_entries() {
        let dir = TempDir::new().expect("tempdir");
        install(dir.path(), "demo", &[ClientTarget::All], false).expect("install");

        let report = uninstall(dir.path(), &[ClientTarget::Claude], false).expect("uninstall");

        assert_eq!(report.clients, vec![ClientTarget::Claude]);
        assert!(!dir.path().join(CLAUDE_MCP_PATH).exists());
        assert!(dir.path().join(CURSOR_MCP_PATH).is_file());
        assert!(dir.path().join(CODEX_CONFIG_PATH).is_file());
        assert!(dir.path().join(MCP_MANIFEST_PATH).is_file());
        assert!(dir.path().join(crate::hooks::HOOKS_MANIFEST_PATH).is_file());
        assert!(dir.path().join(AGENT_GUIDE_PATH).is_file());
    }

    #[test]
    fn json_merge_preserves_user_key_order() {
        let dir = TempDir::new().expect("tempdir");
        std::fs::write(
            dir.path().join(CLAUDE_MCP_PATH),
            r#"{"zeta":1,"alpha":2,"mcpServers":{"other":{"command":"other"}}}"#,
        )
        .expect("config");

        install(dir.path(), "demo", &[ClientTarget::Claude], false).expect("install");

        let merged = std::fs::read_to_string(dir.path().join(CLAUDE_MCP_PATH)).expect("merged");
        let zeta = merged.find("\"zeta\"").expect("zeta");
        let alpha = merged.find("\"alpha\"").expect("alpha");
        let servers = merged.find("\"mcpServers\"").expect("servers");
        assert!(zeta < alpha && alpha < servers);
    }

    #[test]
    fn uninstall_conflict_is_detected_before_any_removal() {
        let dir = TempDir::new().expect("tempdir");
        install(dir.path(), "demo", &[ClientTarget::All], false).expect("install");
        std::fs::write(
            dir.path().join(CURSOR_MCP_PATH),
            r#"{"mcpServers":{"texo":{"command":"other"}}}"#,
        )
        .expect("conflict");
        let before = fingerprint_files(dir.path());

        let error = uninstall(dir.path(), &[], false).expect_err("conflict");

        assert!(error.to_string().contains("not managed"));
        assert_eq!(fingerprint_files(dir.path()), before);
    }

    #[cfg(unix)]
    #[test]
    fn install_refuses_symlinked_client_paths_and_preserves_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt as _};

        let linked = TempDir::new().expect("linked root");
        let outside = TempDir::new().expect("outside");
        symlink(outside.path(), linked.path().join(".cursor")).expect("symlink");
        let before = fingerprint_files(outside.path());
        let error = install(linked.path(), "demo", &[ClientTarget::All], false)
            .expect_err("symlink must fail");
        assert!(error.to_string().contains("symbolic link"));
        assert_eq!(fingerprint_files(outside.path()), before);
        assert!(!linked.path().join(".texo").exists());

        let permissions = TempDir::new().expect("permissions root");
        std::fs::write(permissions.path().join(CLAUDE_MCP_PATH), "{}\n").expect("config");
        let mut mode = std::fs::metadata(permissions.path().join(CLAUDE_MCP_PATH))
            .expect("metadata")
            .permissions();
        mode.set_mode(0o600);
        std::fs::set_permissions(permissions.path().join(CLAUDE_MCP_PATH), mode)
            .expect("permissions");
        install(permissions.path(), "demo", &[ClientTarget::Claude], false).expect("install");
        assert_eq!(
            std::fs::metadata(permissions.path().join(CLAUDE_MCP_PATH))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
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
