//! Stable installer reports re-exported by the install facade.

use serde::Serialize;

use super::{ChangeAction, ClientJournalRoute, ClientTarget};

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
