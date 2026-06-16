//! Staleness diagnostic types.

use serde::{Deserialize, Serialize};

use crate::types::ids::{ClaimId, WorkspaceId};
use crate::types::receipt::ReceiptView;

/// Severity of a staleness diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum DiagnosticSeverity {
    /// Stale or superseded claim.
    Warning,
}

/// Source pointer for a superseding claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticSource {
    /// Source path.
    pub path: String,
    /// Line start.
    pub line_start: u32,
}

/// One staleness diagnostic for editor/CLI/MCP surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaleDiagnostic {
    /// File containing stale prose.
    pub file: String,
    /// Start line.
    pub line_start: u32,
    /// End line.
    pub line_end: u32,
    /// Severity.
    pub severity: DiagnosticSeverity,
    /// Human-readable message.
    pub message: String,
    /// Stale claim id.
    pub claim_id: ClaimId,
    /// Superseding claim id if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<ClaimId>,
    /// Superseding claim source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<DiagnosticSource>,
    /// Supersession receipt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ReceiptView>,
}

/// Full staleness report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StalenessReport {
    /// Workspace id.
    pub workspace_id: WorkspaceId,
    /// Checked path.
    pub checked_path: String,
    /// Replay frontier sequence.
    pub replayed_through_sequence: u64,
    /// Diagnostics.
    pub diagnostics: Vec<StaleDiagnostic>,
}
