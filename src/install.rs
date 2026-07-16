//! Idempotent workspace appliance installation.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde::Serialize;

use crate::error::TexoError;
use crate::topology::{JournalEntry, JournalRole, ReplicaMode};

mod adapter;
mod entry;
mod filesystem;
mod model;

use adapter::{
    canonical_manifest, client_path, managed_created_paths, managed_journal_id,
    managed_server_entry, managed_workspace_id, merge_client_adapter, preflight_remove_client,
    remove_client, resolve_clients, upsert_agent_guide, AGENT_MARKER_END, AGENT_MARKER_START,
};
use filesystem::{
    ensure_safe_managed_path, remove_managed_file, remove_marked_block, with_newline, write_managed,
};

pub use entry::install;
pub use model::{InstallChange, InstallReport};

/// Canonical client-neutral MCP manifest.
pub const MCP_MANIFEST_PATH: &str = ".texo/mcp.json";
/// Root agent guidance file updated through one managed marker block.
pub const AGENT_GUIDE_PATH: &str = "AGENTS.md";

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
    let plan = prepare_install_plan(root, workspace_id, journal_id, requested)?;
    let changes = apply_install_plan(root, workspace_id, journal_id, dry_run, &plan)?;
    Ok(InstallReport {
        schema: "texo.install.v2",
        root: root.display().to_string(),
        workspace_id: workspace_id.to_string(),
        dry_run,
        clients: plan.clients,
        routes: plan.routes,
        changes,
    })
}

struct InstallPlan {
    clients: Vec<ClientTarget>,
    created_paths: BTreeSet<String>,
    config_path: PathBuf,
    config_existed: bool,
    decision: crate::surfaces::bootstrap::BootstrapDecision,
    root_config: crate::config::TexoRootConfig,
    routes: Vec<ClientJournalRoute>,
    topology_changed: bool,
    previous_server: Option<serde_json::Value>,
}

fn prepare_install_plan(
    root: &Path,
    workspace_id: &str,
    journal_id: Option<&str>,
    requested: &[ClientTarget],
) -> Result<InstallPlan, TexoError> {
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
    Ok(InstallPlan {
        clients,
        created_paths,
        config_path,
        config_existed,
        decision,
        root_config,
        routes,
        topology_changed,
        previous_server,
    })
}

fn apply_install_plan(
    root: &Path,
    workspace_id: &str,
    journal_id: Option<&str>,
    dry_run: bool,
    plan: &InstallPlan,
) -> Result<Vec<InstallChange>, TexoError> {
    let mut changes = Vec::new();
    changes.push(apply_install_topology(root, workspace_id, dry_run, plan)?);
    append_managed_surface_changes(&mut changes, root, workspace_id, journal_id, dry_run, plan)?;
    Ok(changes)
}

fn apply_install_topology(
    root: &Path,
    workspace_id: &str,
    dry_run: bool,
    plan: &InstallPlan,
) -> Result<InstallChange, TexoError> {
    if !dry_run {
        crate::surfaces::bootstrap::ensure_workspace(root, workspace_id, &plan.decision)?;
        if plan.topology_changed {
            plan.root_config
                .save(&plan.config_path)
                .map_err(|error| TexoError::Config {
                    detail: error.to_string(),
                    source: Some(Box::new(error)),
                })?;
        }
        for route in &plan.routes {
            let _report =
                crate::replication::refresh_reader(root, Some(workspace_id), &route.journal_id)?;
        }
    }
    Ok(InstallChange {
        path: ".texo/config.toml".to_string(),
        action: if !plan.config_existed {
            ChangeAction::Created
        } else if plan.topology_changed {
            ChangeAction::Updated
        } else {
            ChangeAction::Unchanged
        },
    })
}

fn append_managed_surface_changes(
    changes: &mut Vec<InstallChange>,
    root: &Path,
    workspace_id: &str,
    journal_id: Option<&str>,
    dry_run: bool,
    plan: &InstallPlan,
) -> Result<(), TexoError> {
    let canonical = serde_json::to_vec_pretty(&canonical_manifest(
        workspace_id,
        journal_id,
        &plan.created_paths,
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

    for client in &plan.clients {
        let Some(change) = merge_client_adapter(
            root,
            workspace_id,
            *client,
            route_for(&plan.routes, *client),
            plan.previous_server.as_ref(),
            dry_run,
        )?
        else {
            continue;
        };
        changes.push(change);
    }
    changes.push(upsert_agent_guide(root, workspace_id, dry_run)?);
    Ok(())
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

fn config_error(path: &str, detail: &str) -> TexoError {
    TexoError::Config {
        detail: format!("{path}: {detail}"),
        source: None,
    }
}

/// Return the managed adapter paths for doctor diagnostics.
#[must_use]
pub fn adapter_paths() -> [PathBuf; 4] {
    adapter::paths()
}

#[cfg(test)]
mod tests;
