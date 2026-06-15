//! Ingest mode and result types.

use crate::types::receipt::ReceiptView;

/// Whether ingest should commit to the journal or only plan appends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestMode {
    /// Describe appends without writing.
    DryRun,
    /// Append events to the journal.
    Commit,
}

/// Planned ingest operations (no journal writes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestPlan {
    /// Number of sources that would be observed.
    pub sources_observed: usize,
    /// Number of claims that would be recorded.
    pub claims_recorded: usize,
    /// Workspace id.
    pub workspace_id: String,
}

/// Committed ingest with append receipts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestCommitted {
    /// Number of sources observed.
    pub sources_observed: usize,
    /// Number of claims recorded.
    pub claims_recorded: usize,
    /// Workspace id.
    pub workspace_id: String,
    /// Append receipts emitted.
    pub receipts: Vec<ReceiptView>,
}

/// JSON-serializable ingest summary.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestReport {
    /// Workspace id.
    pub workspace_id: String,
    /// Sources observed count.
    pub sources_observed: usize,
    /// Claims recorded count.
    pub claims_recorded: usize,
    /// Receipts when committed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub receipts: Vec<ReceiptView>,
}

impl From<IngestCommitted> for IngestReport {
    fn from(value: IngestCommitted) -> Self {
        Self {
            workspace_id: value.workspace_id,
            sources_observed: value.sources_observed,
            claims_recorded: value.claims_recorded,
            receipts: value.receipts,
        }
    }
}

impl From<IngestPlan> for IngestReport {
    fn from(value: IngestPlan) -> Self {
        Self {
            workspace_id: value.workspace_id,
            sources_observed: value.sources_observed,
            claims_recorded: value.claims_recorded,
            receipts: Vec::new(),
        }
    }
}
