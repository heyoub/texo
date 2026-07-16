use super::super::common::claim_record_receipts;
use super::{DEFAULT_RELATE_CACHE, ENV_RELATE_CACHE};
use crate::claims::workspace::WorkspaceView;
use crate::error::TexoError;
use crate::events::coordinate::{entity_for_claim, scope_for_workspace};
use crate::events::ids::{ClaimId, SourceId};
use crate::semantics::pipeline::{
    receipt_view, CandidateCursor, ClaimStatus as SemanticClaimStatus,
    ClaimView as SemanticClaimView, ParallelRelateOptions, RelateOutcome, RelateTemporalPolicy,
    RelateThresholds,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(super) struct SemanticRelateOutput {
    pub(super) outcome: crate::semantics::pipeline::RelateOutcome,
    pub(super) cache_keys: BTreeMap<(String, String), String>,
    pub(super) judge_fingerprint: String,
}

#[derive(Clone, Copy)]
pub(super) struct RelateBackendContext<'a> {
    pub(super) root: &'a Path,
    pub(super) gateway: Option<&'a crate::gateway::GatewayConfig>,
    pub(super) claims: &'a [(ClaimId, SemanticClaimView)],
    pub(super) thresholds: RelateThresholds,
    pub(super) settled: &'a BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    pub(super) temporal: &'a RelateTemporalPolicy,
    pub(super) budget: std::time::Duration,
    pub(super) candidate_pair_budget: usize,
    pub(super) candidate_cursor: CandidateCursor,
}

#[cfg(feature = "openrouter")]
pub(super) fn relate_with_backends(
    context: RelateBackendContext<'_>,
) -> Result<SemanticRelateOutput, TexoError> {
    use crate::extract::cache::CachingRelater;
    use crate::semantics::openrouter::{OpenRouterEmbedder, OpenRouterRelater};
    use crate::semantics::pipeline::relate_claims_settled_parallel_temporal;
    use crate::semantics::ClaimRelater as _;

    let RelateBackendContext {
        root,
        gateway,
        claims,
        thresholds,
        settled,
        temporal,
        budget,
        candidate_pair_budget,
        candidate_cursor,
    } = context;
    let embedder = OpenRouterEmbedder::new(None, gateway).map_err(semantic_error)?;
    let cache_dir = std::env::var_os(ENV_RELATE_CACHE)
        .map_or_else(|| root.join(DEFAULT_RELATE_CACHE), PathBuf::from);
    let caching_relater = CachingRelater::new(
        OpenRouterRelater::new(None, gateway).map_err(semantic_error)?,
        cache_dir,
    );
    let judge_fingerprint = caching_relater.fingerprint();
    // Judge calls are independent network waits; fan out across workers and
    // reassemble in pair order so settlement stays byte-identical. 4 default
    // workers keeps provider pressure polite; clamp guards misconfiguration.
    let concurrency = std::env::var("TEXO_RELATE_CONCURRENCY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(4)
        .clamp(1, 16);
    let relation_output = relate_claims_settled_parallel_temporal(
        claims,
        &embedder,
        &caching_relater,
        thresholds,
        settled,
        ParallelRelateOptions {
            temporal,
            budget,
            concurrency,
            candidate_pair_budget,
            candidate_cursor,
        },
    )
    .map_err(semantic_error)?;
    let cache_keys = relate_cache_keys(&caching_relater, claims, &relation_output);
    Ok(SemanticRelateOutput {
        outcome: relation_output,
        cache_keys,
        judge_fingerprint,
    })
}

#[cfg(not(feature = "openrouter"))]
pub(super) fn relate_with_backends(
    _context: RelateBackendContext<'_>,
) -> Result<SemanticRelateOutput, TexoError> {
    Err(TexoError::Semantics {
        backend: "openrouter".to_string(),
        detail: "openrouter feature is disabled".to_string(),
    })
}

#[cfg(feature = "openrouter")]
pub(super) fn semantic_error(error: impl std::error::Error + Send + Sync + 'static) -> TexoError {
    TexoError::Semantics {
        backend: "openrouter".to_string(),
        detail: crate::error::error_chain(&error),
    }
}

pub(super) fn semantic_claims_from_view(
    view: &WorkspaceView,
) -> Result<Vec<(ClaimId, SemanticClaimView)>, TexoError> {
    let receipts = claim_record_receipts()?;
    let mut claims = Vec::new();
    for claim in &view.claims {
        if claim.status != crate::claims::status::ClaimStatus::Current {
            continue;
        }
        let claim_id = ClaimId::try_from(claim.card.claim_id.as_str())?;
        let source_id = SourceId::try_from(claim.card.source_id.as_str())?;
        let receipt =
            receipts
                .get(&claim.card.claim_id)
                .ok_or_else(|| TexoError::MissingEntity {
                    entity: entity_for_claim(&claim.card.claim_id),
                })?;
        let supersedes = claim
            .supersedes
            .iter()
            .map(|id| ClaimId::try_from(id.as_str()))
            .collect::<Result<Vec<_>, _>>()?;
        let superseded_by = claim
            .card
            .superseded_by
            .as_deref()
            .map(ClaimId::try_from)
            .transpose()?;
        claims.push((
            claim_id.clone(),
            SemanticClaimView {
                claim_id,
                workspace_id: claim.card.workspace_id.clone(),
                source_id,
                source_path: claim.card.source_path.clone(),
                line_start: claim.card.line_start,
                line_end: claim.card.line_end,
                text: claim.card.text.clone(),
                normalized_text: claim.card.normalized_text.clone(),
                subject_hint: claim.card.subject_hint.clone().unwrap_or_default(),
                predicate_hint: claim.card.predicate_hint.clone().unwrap_or_default(),
                object_hint: claim.card.object_hint.clone().unwrap_or_default(),
                confidence_ppm: claim.card.confidence_ppm,
                extractor_kind: claim.card.extractor_kind.clone(),
                status: SemanticClaimStatus::Current,
                receipt: receipt_view(
                    0,
                    receipt.sequence,
                    "ClaimRecorded",
                    &scope_for_workspace(&view.workspace_id),
                    &entity_for_claim(&claim.card.claim_id),
                ),
                supersedes,
                superseded_by,
            },
        ));
    }
    claims.sort_by(|left, right| {
        left.1
            .receipt
            .sequence
            .get()
            .cmp(&right.1.receipt.sequence.get())
            .then_with(|| left.0.as_str().cmp(right.0.as_str()))
    });
    Ok(claims)
}

#[cfg(feature = "openrouter")]
fn relate_cache_keys<R: crate::semantics::ClaimRelater>(
    caching_relater: &crate::extract::cache::CachingRelater<R>,
    claims: &[(ClaimId, SemanticClaimView)],
    relation_output: &RelateOutcome,
) -> BTreeMap<(String, String), String> {
    let by_id = claims
        .iter()
        .map(|(id, view)| (id.to_string(), view))
        .collect::<BTreeMap<_, _>>();
    relation_output
        .judgments()
        .iter()
        .filter_map(|judgment| {
            let old_view = by_id.get(judgment.older_claim.as_str())?;
            let new_view = by_id.get(judgment.newer_claim.as_str())?;
            Some((
                (
                    judgment.older_claim.to_string(),
                    judgment.newer_claim.to_string(),
                ),
                caching_relater.cache_key(&old_view.text, &new_view.text),
            ))
        })
        .collect()
}
