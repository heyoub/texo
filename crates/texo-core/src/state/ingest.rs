//! Ingest mode and result types.

use crate::types::ids::WorkspaceId;
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
    pub workspace_id: WorkspaceId,
}

/// Committed ingest with append receipts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestCommitted {
    /// Number of sources observed.
    pub sources_observed: usize,
    /// Number of claims recorded.
    pub claims_recorded: usize,
    /// Workspace id.
    pub workspace_id: WorkspaceId,
    /// Append receipts emitted.
    pub receipts: Vec<ReceiptView>,
}

/// JSON-serializable ingest summary.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestReport {
    /// Workspace id.
    pub workspace_id: WorkspaceId,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::receipt::receipt_view;

    fn workspace() -> WorkspaceId {
        WorkspaceId::new("demo").expect("workspace id")
    }

    #[test]
    fn committed_report_carries_receipts() {
        let committed = IngestCommitted {
            sources_observed: 2,
            claims_recorded: 5,
            workspace_id: workspace(),
            receipts: vec![receipt_view(
                1,
                1,
                "SourceObserved",
                "workspace:demo",
                "source:a",
            )],
        };
        let report: IngestReport = committed.into();
        assert_eq!(report.sources_observed, 2);
        assert_eq!(report.claims_recorded, 5);
        assert_eq!(report.workspace_id.as_str(), "demo");
        assert_eq!(report.receipts.len(), 1);
        // The conversion must carry the receipt through intact, not just its
        // count: kind, scope, and sequence must survive.
        let receipt = &report.receipts[0];
        assert_eq!(receipt.kind, "SourceObserved");
        assert_eq!(receipt.scope, "workspace:demo");
        assert_eq!(receipt.sequence.get(), 1);
    }

    #[test]
    fn plan_report_has_no_receipts() {
        let plan = IngestPlan {
            sources_observed: 3,
            claims_recorded: 7,
            workspace_id: workspace(),
        };
        let report: IngestReport = plan.into();
        assert_eq!(report.sources_observed, 3);
        assert_eq!(report.claims_recorded, 7);
        assert!(report.receipts.is_empty());
    }

    #[test]
    fn report_skips_empty_receipts_in_json() {
        // `skip_serializing_if = "Vec::is_empty"`: a dry-run report must not emit
        // a `receipts` key, but a committed one must.
        let plan_report: IngestReport = IngestPlan {
            sources_observed: 1,
            claims_recorded: 1,
            workspace_id: workspace(),
        }
        .into();
        let plan_json = serde_json::to_string(&plan_report).expect("serialize plan");
        assert!(
            !plan_json.contains("receipts"),
            "dry-run report must omit empty receipts, got: {plan_json}"
        );

        let committed_report: IngestReport = IngestCommitted {
            sources_observed: 1,
            claims_recorded: 1,
            workspace_id: workspace(),
            receipts: vec![receipt_view(
                1,
                1,
                "SourceObserved",
                "workspace:demo",
                "source:a",
            )],
        }
        .into();
        let committed_json = serde_json::to_string(&committed_report).expect("serialize committed");
        assert!(committed_json.contains("receipts"));
    }

    #[test]
    fn ingest_mode_is_copy_and_eq() {
        assert_eq!(IngestMode::DryRun, IngestMode::DryRun);
        assert_ne!(IngestMode::DryRun, IngestMode::Commit);
    }
}
