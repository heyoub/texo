use serde::Serialize;

#[derive(Debug, Serialize)]
pub(crate) struct AgentClaimRow {
    pub(crate) claim_id: String,
    pub(crate) status: crate::claims::status::ClaimStatus,
    pub(crate) subject_hint: Option<String>,
    pub(crate) text: String,
    pub(crate) source: AgentSourceRow,
    pub(crate) receipt: AgentReceiptRow,
    pub(crate) supersedes: Vec<String>,
    pub(crate) superseded_by: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentSourceRow {
    pub(crate) source_id: String,
    pub(crate) path: String,
    pub(crate) line_start: u32,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AgentReceiptRow {
    pub(crate) event_id: String,
    pub(crate) sequence: u64,
}
