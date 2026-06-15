//! Ingest planning and execution helpers.

use std::collections::HashSet;
use std::path::Path;

use crate::config::TexoConfig;
use crate::events::payloads::{ClaimRecorded, ClaimSuperseded, SourceObserved};
use crate::extract::extract_claims;
use crate::journal::JournalError;
use crate::replay::state::ClaimView;
use crate::source::{collect_markdown_files, MarkdownDocument};
use crate::stale::check::infer_supersessions;
use crate::types::ids::{ClaimId, SourceId, WorkspaceId};

/// Internal ingest plan with append payloads.
#[derive(Debug, Clone)]
pub struct IngestPlanInternal {
    /// Workspace id string.
    pub workspace_id: String,
    /// Sources observed count.
    pub sources_observed: usize,
    /// Claims recorded count.
    pub claims_recorded: usize,
    /// Planned append actions in order.
    pub actions: Vec<PlannedAction>,
}

/// One planned append action.
#[derive(Debug, Clone)]
pub enum PlannedAction {
    /// Source observed payload.
    Source(SourceObserved),
    /// Claim recorded payload.
    Claim(ClaimRecorded),
    /// Supersession payload.
    Supersede(ClaimSuperseded),
}

/// Build an ingest plan by scanning markdown sources.
pub fn plan_sources(
    input: &Path,
    _config: &TexoConfig,
    workspace: &WorkspaceId,
    observed_at_ms: u64,
    existing_hashes: &HashSet<String>,
) -> Result<IngestPlanInternal, JournalError> {
    let root = if input.is_dir() {
        input
    } else {
        input.parent().unwrap_or_else(|| Path::new("."))
    };

    let files = collect_markdown_files(input).map_err(|e| JournalError::Domain(e.to_string()))?;
    let mut actions = Vec::new();
    let mut seen_source_hashes = HashSet::new();
    let mut claims_recorded = 0usize;
    let mut claim_views: Vec<(ClaimId, ClaimView)> = Vec::new();
    let mut sequence = 0u64;

    for path in files {
        let doc = MarkdownDocument::from_path(&path, root)
            .map_err(|e| JournalError::Domain(e.to_string()))?;
        if seen_source_hashes.contains(&doc.body_hash_hex)
            || existing_hashes.contains(&doc.body_hash_hex)
        {
            continue;
        }
        seen_source_hashes.insert(doc.body_hash_hex.clone());

        let source_payload = SourceObserved {
            source_id: doc.source_id.clone(),
            workspace_id: workspace.to_string(),
            source_kind: "markdown".to_string(),
            path: doc.path.clone(),
            body_hash_hex: doc.body_hash_hex.clone(),
            observed_at_ms,
        };
        actions.push(PlannedAction::Source(source_payload));

        let source_id = SourceId::try_from(doc.source_id.as_str())
            .map_err(|e| JournalError::Domain(e.to_string()))?;
        let claims = extract_claims(&doc, &source_id, workspace.as_str(), observed_at_ms)
            .map_err(|e| JournalError::Domain(e.to_string()))?;
        claims_recorded += claims.len();
        for claim in claims {
            sequence += 1;
            let claim_id = ClaimId::try_from(claim.payload.claim_id.as_str())
                .map_err(|e| JournalError::Domain(e.to_string()))?;
            let view = claim_view_from_payload(&claim.payload, sequence);
            claim_views.push((claim_id, view));
            actions.push(PlannedAction::Claim(claim.payload));
        }
    }

    for (old_id, new_id, reason) in infer_supersessions(&claim_views) {
        actions.push(PlannedAction::Supersede(ClaimSuperseded {
            old_claim_id: old_id.to_string(),
            new_claim_id: new_id.to_string(),
            workspace_id: workspace.to_string(),
            reason,
            decided_by: "texo-ingest".to_string(),
            observed_at_ms,
        }));
    }

    Ok(IngestPlanInternal {
        workspace_id: workspace.to_string(),
        sources_observed: seen_source_hashes.len(),
        claims_recorded,
        actions,
    })
}

fn claim_view_from_payload(payload: &ClaimRecorded, sequence: u64) -> ClaimView {
    use crate::replay::state::ClaimView;
    use crate::state::claim_lifecycle::initial_claim_status;
    use crate::types::receipt::receipt_view;

    ClaimView {
        claim_id: ClaimId::try_from(payload.claim_id.as_str()).expect("claim id"),
        workspace_id: payload.workspace_id.clone(),
        source_id: SourceId::try_from(payload.source_id.as_str()).expect("source id"),
        source_path: payload.source_path.clone(),
        line_start: payload.line_start,
        line_end: payload.line_end,
        text: payload.text.clone(),
        normalized_text: payload.normalized_text.clone(),
        subject_hint: payload.subject_hint.clone(),
        predicate_hint: payload.predicate_hint.clone(),
        object_hint: payload.object_hint.clone(),
        confidence_ppm: payload.confidence_ppm,
        extractor_kind: payload.extractor_kind.clone(),
        status: initial_claim_status(),
        receipt: receipt_view(
            0,
            sequence,
            "ClaimRecorded",
            &format!("workspace:{}", payload.workspace_id),
            &payload.claim_id,
        ),
        supersedes: Vec::new(),
        superseded_by: None,
    }
}
