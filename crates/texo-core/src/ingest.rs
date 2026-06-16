//! Ingest planning and execution helpers.

use std::collections::HashSet;
use std::path::Path;

use crate::config::WorkspaceConfig;
use crate::events::payloads::{ClaimRecorded, ClaimSuperseded, SourceObserved};
use crate::extract::{
    extract_claims, extract_via_cmd, ExtractClaimsFn, ExtractError, ExtractedClaim,
};
use crate::journal::JournalError;
use crate::replay::state::ClaimView;
use crate::source::{collect_markdown_files, MarkdownDocument};
use crate::stale::check::infer_supersessions;
use crate::types::ids::{ClaimId, SourceId, WorkspaceId};

/// Internal ingest plan with append payloads.
#[derive(Debug, Clone)]
pub struct IngestPlanInternal {
    /// Workspace id.
    pub workspace_id: WorkspaceId,
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

/// Borrowed inputs for ingest planning.
///
/// `historical_claims` are the workspace's currently-active claims (loaded from
/// the journal) included as supersession candidates; only new-batch claims may
/// supersede them. `existing_edges` are already-recorded
/// `(old_claim_id, new_claim_id)` pairs that must not be duplicated.
#[derive(Debug, Clone, Copy)]
pub struct PlanInput<'a> {
    /// File or directory to scan for markdown sources.
    pub input: &'a Path,
    /// Resolved workspace configuration.
    pub config: &'a WorkspaceConfig,
    /// Target workspace id.
    pub workspace: &'a WorkspaceId,
    /// Wall-clock observation timestamp (ms) stamped on emitted events.
    pub observed_at_ms: u64,
    /// Source body hashes already present so duplicates are skipped.
    pub existing_hashes: &'a HashSet<String>,
    /// Workspace root used to resolve relative paths.
    pub root: &'a Path,
    /// Currently-active workspace claims offered as supersession candidates.
    pub historical_claims: &'a [(ClaimId, ClaimView)],
    /// Already-recorded supersession edges that must not be re-emitted.
    pub existing_edges: &'a HashSet<(ClaimId, ClaimId)>,
}

/// Build an ingest plan using workspace config (heuristic or optional cmd extractor).
pub fn plan_sources_for_config(input: &PlanInput<'_>) -> Result<IngestPlanInternal, JournalError> {
    plan_sources(input, extract_claims)
}

/// Build an ingest plan by scanning markdown sources.
pub fn plan_sources(
    params: &PlanInput<'_>,
    extract: ExtractClaimsFn,
) -> Result<IngestPlanInternal, JournalError> {
    let &PlanInput {
        input,
        config,
        workspace,
        observed_at_ms,
        existing_hashes,
        root: _,
        historical_claims,
        existing_edges,
    } = params;

    let root_dir = if input.is_dir() {
        input
    } else {
        input.parent().unwrap_or_else(|| Path::new("."))
    };

    let files = collect_markdown_files(input)?;
    let mut actions = Vec::new();
    let mut seen_source_hashes = HashSet::new();
    let mut claims_recorded = 0usize;
    let mut claim_views: Vec<(ClaimId, ClaimView)> = Vec::new();
    let mut sequence = 0u64;

    for path in files {
        let doc = MarkdownDocument::from_path(&path, root_dir)?;
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

        let source_id = SourceId::try_from(doc.source_id.as_str())?;
        // The external extractor runs with its cwd at `root_dir` — the directory
        // `doc.path` was stripped against — so a relative `doc.path` resolves to a
        // real file. (Passing the CLI `root` here mismatches when docs live in a
        // subdirectory: `doc.path` is "sub/x.md" relative to `root_dir`, not root.)
        let claims = extract_for_doc(
            config,
            root_dir,
            &doc,
            &source_id,
            workspace.as_str(),
            observed_at_ms,
            extract,
        )?;
        claims_recorded += claims.len();
        for claim in claims {
            sequence += 1;
            let claim_id = ClaimId::try_from(claim.payload.claim_id.as_str())?;
            let view = claim_view_from_payload(&claim.payload, sequence)?;
            claim_views.push((claim_id, view));
            actions.push(PlannedAction::Claim(claim.payload));
        }
    }

    for (old_id, new_id, reason) in
        infer_supersessions(&claim_views, historical_claims, existing_edges)
    {
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
        workspace_id: workspace.clone(),
        sources_observed: seen_source_hashes.len(),
        claims_recorded,
        actions,
    })
}

fn extract_for_doc(
    config: &WorkspaceConfig,
    root: &Path,
    doc: &MarkdownDocument,
    source_id: &SourceId,
    workspace_id: &str,
    observed_at_ms: u64,
    extract: ExtractClaimsFn,
) -> Result<Vec<ExtractedClaim>, ExtractError> {
    if let Some(cmd) = config.extractor_cmd.as_deref() {
        extract_via_cmd(cmd, doc, source_id, workspace_id, observed_at_ms, root)
    } else {
        extract(doc, source_id, workspace_id, observed_at_ms)
    }
}

fn claim_view_from_payload(
    payload: &ClaimRecorded,
    sequence: u64,
) -> Result<ClaimView, crate::types::IdParseError> {
    use crate::replay::state::ClaimView;
    use crate::state::claim_lifecycle::initial_claim_status;
    use crate::types::receipt::receipt_view;

    Ok(ClaimView {
        claim_id: ClaimId::try_from(payload.claim_id.as_str())?,
        workspace_id: payload.workspace_id.clone(),
        source_id: SourceId::try_from(payload.source_id.as_str())?,
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
    })
}
