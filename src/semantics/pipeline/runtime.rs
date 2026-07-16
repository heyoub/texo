use super::candidate::{
    acquire_sequential, budget_exhausted, prepare_pairs, PairOutcome, PendingPair, PreparedPairs,
};
use super::reduction::{finish_page, PageCompletion};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::events::ids::ClaimId;
use crate::relate::settlement::{PairFailureView, RelationFailureClass, UnresolvedPair};
use crate::semantics::{ClaimRelater, Embedder, SemanticsError};

use super::{
    CandidateCursor, ClaimView, PipelineError, RelateOutcome, RelateTemporalPolicy,
    RelateThresholds, DEFAULT_CANDIDATE_PAIR_BUDGET,
};

/// Relate claims by a single richer judgment per candidate pair.
///
/// This is the primary relating entry point. A 3-way NLI label cannot distinguish
/// a value replacement from a genuine disagreement — measured against real models,
/// *both* are mutual contradiction, and embeddings alone cannot tell "Friday
/// deploy" from "Friday release". A [`ClaimRelater`] answers both questions
/// (shared subject? update or conflict?) at once.
///
/// Candidate generation is a deterministic page over the global pair space:
/// 1. Embed every claim once.
/// 2. Examine at most `candidate_pair_budget` raw pair slots from the supplied
///    [`CandidateCursor`].
/// 3. Judge only pairs clearing the lower configured cosine floor. This is a
///    recall gate; the relater performs semantic classification.
/// 4. Order each surviving pair oldest→newest (by `receipt.sequence`, index as a
///    deterministic tiebreak) and ask the relater how the newer relates to the
///    older. Identical-normalized-text pairs are skipped as duplicates.
/// 5. [`ClaimRelation::Supersedes`] → the older is superseded; among all claims
///    that supersede it, the **newest** wins (one canonical edge per stale claim).
/// 6. [`ClaimRelation::Conflict`] → a candidate conflict, kept only if **neither**
///    side was superseded in step 4 (a superseded claim is no longer current).
///
/// The cursor counts raw pair slots before similarity and duplicate-text gates,
/// so settled pairs and filtered pairs consume the same bounded work as live
/// pairs. Partial pages cannot expose derived authority and return the exact
/// cursor required to resume.
///
/// Pure and deterministic for a given input order and backend behavior:
/// embedding, pair enumeration, and all output ordering depend only on slice
/// order and journal sequence, never on hash-map iteration order.
///
/// # Errors
///
/// Returns [`PipelineError::Semantics`] when the [`Embedder`] fails to embed the
/// claim texts or the [`ClaimRelater`] fails to judge a candidate pair.
pub fn relate_claims(
    claims: &[(ClaimId, ClaimView)],
    embedder: &dyn Embedder,
    relater: &dyn ClaimRelater,
    thresholds: RelateThresholds,
) -> Result<RelateOutcome, PipelineError> {
    relate_claims_with_settled(
        claims,
        embedder,
        relater,
        thresholds,
        &BTreeMap::new(),
        Duration::MAX,
    )
}

/// Relate claims while reusing journal-authoritative verdicts and enforcing a
/// global wall-clock budget. Verdicts are acquired sequentially in pair order.
///
/// # Errors
/// Embedding failures remain fatal because no candidate substrate exists.
pub fn relate_claims_with_settled(
    claims: &[(ClaimId, ClaimView)],
    embedder: &dyn Embedder,
    relater: &dyn ClaimRelater,
    thresholds: RelateThresholds,
    settled: &BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    budget: Duration,
) -> Result<RelateOutcome, PipelineError> {
    relate_claims_with_settled_temporal(
        claims,
        embedder,
        relater,
        thresholds,
        settled,
        &RelateTemporalPolicy::default(),
        budget,
    )
}

/// Relate claims with journal authority and replayed source-order evidence.
///
/// # Errors
/// Embedding failures remain fatal because no candidate substrate exists.
pub fn relate_claims_with_settled_temporal(
    claims: &[(ClaimId, ClaimView)],
    embedder: &dyn Embedder,
    relater: &dyn ClaimRelater,
    thresholds: RelateThresholds,
    settled: &BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    temporal: &RelateTemporalPolicy,
    budget: Duration,
) -> Result<RelateOutcome, PipelineError> {
    let started = Instant::now();
    let Some(prepared) = prepare_pairs(
        claims,
        embedder,
        thresholds,
        settled,
        temporal,
        DEFAULT_CANDIDATE_PAIR_BUDGET,
        CandidateCursor::start(),
    )?
    else {
        return Ok(RelateOutcome::default());
    };
    let PreparedPairs {
        pending,
        examined_pairs,
        next_cursor,
        claim_clusters,
    } = prepared;
    let outcomes = pending
        .iter()
        .map(|pair| acquire_sequential(claims, pair, relater, settled, started, budget))
        .collect::<Vec<_>>();
    Ok(finish_page(
        claims,
        pending,
        outcomes,
        temporal,
        &PageCompletion {
            settled,
            claim_clusters: &claim_clusters,
            examined_pairs,
            candidate_pair_budget: DEFAULT_CANDIDATE_PAIR_BUDGET,
            next_cursor,
        },
    ))
}

/// Like [`relate_claims_with_settled`], but live judge calls fan out across
/// `concurrency` worker threads. Journal-authoritative verdicts resolve on the
/// calling thread; workers receive only genuinely unjudged pairs; results are
/// reassembled in pending order, so decision reduction and journal append
/// order are byte-identical to the sequential path for the same verdict set.
/// Under a wall budget, WHICH pairs get cut off is timing-dependent (as it
/// already is sequentially); a completed pass is fully deterministic.
///
/// # Errors
/// Embedding failures remain fatal because no candidate substrate exists.
pub fn relate_claims_settled_parallel(
    claims: &[(ClaimId, ClaimView)],
    embedder: &dyn Embedder,
    relater: &(dyn ClaimRelater + Sync),
    thresholds: RelateThresholds,
    settled: &BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    budget: Duration,
    concurrency: usize,
) -> Result<RelateOutcome, PipelineError> {
    relate_claims_settled_parallel_temporal(
        claims,
        embedder,
        relater,
        thresholds,
        settled,
        ParallelRelateOptions {
            temporal: &RelateTemporalPolicy::default(),
            budget,
            concurrency,
            candidate_pair_budget: DEFAULT_CANDIDATE_PAIR_BUDGET,
            candidate_cursor: CandidateCursor::start(),
        },
    )
}

/// Runtime controls for parallel semantic settlement.
#[derive(Debug, Clone, Copy)]
pub struct ParallelRelateOptions<'a> {
    /// Replayed source-order policy.
    pub temporal: &'a RelateTemporalPolicy,
    /// Global wall-clock budget.
    pub budget: Duration,
    /// Maximum live judge workers.
    pub concurrency: usize,
    /// Hard ceiling for raw global pair slots examined by this pass.
    pub candidate_pair_budget: usize,
    /// Cursor returned by the prior partial result.
    pub candidate_cursor: CandidateCursor,
}

/// Parallel relation acquisition with journal authority and replayed
/// source-order evidence.
///
/// # Errors
/// Embedding failures remain fatal because no candidate substrate exists.
pub fn relate_claims_settled_parallel_temporal(
    claims: &[(ClaimId, ClaimView)],
    embedder: &dyn Embedder,
    relater: &(dyn ClaimRelater + Sync),
    thresholds: RelateThresholds,
    settled: &BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    options: ParallelRelateOptions<'_>,
) -> Result<RelateOutcome, PipelineError> {
    let ParallelRelateOptions {
        temporal,
        budget,
        concurrency,
        candidate_pair_budget,
        candidate_cursor,
    } = options;
    let started = Instant::now();
    let Some(prepared) = prepare_pairs(
        claims,
        embedder,
        thresholds,
        settled,
        temporal,
        candidate_pair_budget,
        candidate_cursor,
    )?
    else {
        return Ok(RelateOutcome::default());
    };
    let PreparedPairs {
        pending,
        examined_pairs,
        next_cursor,
        claim_clusters,
    } = prepared;
    if concurrency <= 1 {
        let outcomes = pending
            .iter()
            .map(|pair| acquire_sequential(claims, pair, relater, settled, started, budget))
            .collect::<Vec<_>>();
        return Ok(finish_page(
            claims,
            pending,
            outcomes,
            temporal,
            &PageCompletion {
                settled,
                claim_clusters: &claim_clusters,
                examined_pairs,
                candidate_pair_budget,
                next_cursor,
            },
        ));
    }

    let outcomes = acquire_parallel(
        claims,
        &pending,
        relater,
        settled,
        started,
        budget,
        concurrency,
    );

    let outcomes = outcomes
        .into_iter()
        .map(|slot| slot.unwrap_or(PairOutcome::Failed(budget_exhausted())))
        .collect::<Vec<_>>();
    Ok(finish_page(
        claims,
        pending,
        outcomes,
        temporal,
        &PageCompletion {
            settled,
            claim_clusters: &claim_clusters,
            examined_pairs,
            candidate_pair_budget,
            next_cursor,
        },
    ))
}

fn acquire_parallel(
    claims: &[(ClaimId, ClaimView)],
    pending: &[PendingPair],
    relater: &(dyn ClaimRelater + Sync),
    settled: &BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    started: Instant,
    budget: Duration,
    concurrency: usize,
) -> Vec<Option<PairOutcome>> {
    let mut outcomes = vec![None; pending.len()];
    let mut groups: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    for (idx, pair) in pending.iter().enumerate() {
        if let Some(verdict) = settled.get(&(pair.older.clone(), pair.newer.clone())) {
            outcomes[idx] = Some(PairOutcome::Judged(*verdict, true));
        } else if let Some(class) = pair.temporal_failure {
            outcomes[idx] = Some(PairOutcome::Failed(PairFailureView {
                class,
                endpoint: None,
                status: None,
                attempts: 0,
            }));
        } else {
            let texts = (
                claims[pair.old_idx].1.text.as_str(),
                claims[pair.new_idx].1.text.as_str(),
            );
            groups.entry(texts).or_default().push(idx);
        }
    }
    let mut representatives = groups
        .values()
        .map(|members| members[0])
        .collect::<Vec<_>>();
    representatives.sort_unstable();
    let mut representative_outcomes = run_parallel_jobs(
        claims,
        pending,
        relater,
        started,
        budget,
        concurrency,
        &representatives,
    );
    for members in groups.values() {
        let outcome = representative_outcomes
            .remove(&members[0])
            .unwrap_or(PairOutcome::Failed(budget_exhausted()));
        for &idx in members {
            outcomes[idx] = Some(outcome.clone());
        }
    }
    outcomes
}

fn run_parallel_jobs(
    claims: &[(ClaimId, ClaimView)],
    pending: &[PendingPair],
    relater: &(dyn ClaimRelater + Sync),
    started: Instant,
    budget: Duration,
    concurrency: usize,
    representatives: &[usize],
) -> BTreeMap<usize, PairOutcome> {
    std::thread::scope(|scope| {
        let (job_tx, job_rx) = flume::bounded::<usize>(concurrency);
        let (result_tx, result_rx) = flume::unbounded::<(usize, PairOutcome)>();
        for _ in 0..concurrency {
            let job_rx = job_rx.clone();
            let result_tx = result_tx.clone();
            scope.spawn(move || {
                while let Ok(idx) = job_rx.recv() {
                    let pair = &pending[idx];
                    let outcome = if started.elapsed() >= budget {
                        PairOutcome::Failed(budget_exhausted())
                    } else {
                        match relater
                            .relate(&claims[pair.old_idx].1.text, &claims[pair.new_idx].1.text)
                        {
                            Ok(verdict) => PairOutcome::Judged(verdict, false),
                            Err(error) => PairOutcome::Failed(classify_pair_failure(&error)),
                        }
                    };
                    if result_tx.send((idx, outcome)).is_err() {
                        return;
                    }
                }
            });
        }
        drop(job_rx);
        drop(result_tx);
        for &idx in representatives {
            if job_tx.send(idx).is_err() {
                break;
            }
        }
        drop(job_tx);
        result_rx.iter().collect()
    })
}

pub(super) fn unresolved_pair(
    old_claim: ClaimId,
    new_claim: ClaimId,
    old_view: &ClaimView,
    new_view: &ClaimView,
    failure: PairFailureView,
) -> UnresolvedPair {
    UnresolvedPair {
        old_claim,
        new_claim,
        old_ref: format!("{}:{}", old_view.source_path, old_view.line_start),
        new_ref: format!("{}:{}", new_view.source_path, new_view.line_start),
        failure,
    }
}

#[cfg(feature = "openrouter")]
pub(crate) fn classify_pair_failure(error: &SemanticsError) -> PairFailureView {
    use crate::semantics::openrouter::BackendError;
    use crate::surfaces::openai::ApiFailureKind;

    let SemanticsError::Backend { source } = error else {
        return generic_pair_failure();
    };
    let Some(backend) = source.downcast_ref::<BackendError>() else {
        return generic_pair_failure();
    };
    match backend {
        BackendError::Http { source, .. } => PairFailureView {
            class: match source.kind {
                ApiFailureKind::HttpStatus => RelationFailureClass::HttpStatus,
                ApiFailureKind::Transport => RelationFailureClass::Transport,
                ApiFailureKind::DeadlineExceeded => RelationFailureClass::Deadline,
                ApiFailureKind::BadResponseJson => RelationFailureClass::Parse,
            },
            endpoint: Some(source.endpoint.to_string()),
            status: source.status,
            attempts: source.attempts,
        },
        BackendError::Truncated { endpoint, .. } => PairFailureView {
            class: RelationFailureClass::Truncated,
            endpoint: Some((*endpoint).to_string()),
            status: None,
            attempts: 1,
        },
        BackendError::Parse { endpoint, .. }
        | BackendError::UnexpectedResponse { endpoint, .. } => PairFailureView {
            class: RelationFailureClass::Parse,
            endpoint: Some((*endpoint).to_string()),
            status: None,
            attempts: 1,
        },
    }
}

#[cfg(not(feature = "openrouter"))]
pub(crate) fn classify_pair_failure(_error: &SemanticsError) -> PairFailureView {
    generic_pair_failure()
}

fn generic_pair_failure() -> PairFailureView {
    PairFailureView {
        class: RelationFailureClass::Transport,
        endpoint: None,
        status: None,
        attempts: 1,
    }
}
