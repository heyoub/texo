use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::events::ids::ClaimId;
use crate::knowledge::TemporalRelation;
use crate::relate::settlement::{PairFailureView, RelationFailureClass};
use crate::semantics::{cosine_similarity, ClaimRelater, Embedder};

use super::runtime::classify_pair_failure;
use super::{
    embedding_text, sequence_rank, CandidateCursor, ClaimView, PipelineError, RelateTemporalPolicy,
    RelateThresholds,
};

/// A candidate pair that survived the bounded page, cosine floor, and
/// duplicate-text check, ordered oldest -> newest.
#[derive(Clone)]
pub(super) struct PendingPair {
    pub(super) old_idx: usize,
    pub(super) new_idx: usize,
    pub(super) older: ClaimId,
    pub(super) newer: ClaimId,
    pub(super) temporal_failure: Option<RelationFailureClass>,
    pub(super) cursor: CandidateCursor,
}

pub(super) struct PreparedPairs {
    pub(super) pending: Vec<PendingPair>,
    pub(super) examined_pairs: usize,
    pub(super) next_cursor: Option<CandidateCursor>,
    pub(super) claim_clusters: Vec<usize>,
}

/// Verdict-acquisition result for one pending pair. Clone so a coalesced text
/// pair's single outcome fans to every logical pair that shares its texts.
#[derive(Clone)]
pub(super) enum PairOutcome {
    Judged(crate::semantics::RelationVerdict, bool),
    Failed(PairFailureView),
}

/// The failure view for a pair the wall budget cut off before it ran.
pub(super) fn budget_exhausted() -> PairFailureView {
    PairFailureView {
        class: RelationFailureClass::BudgetExhausted,
        endpoint: None,
        status: None,
        attempts: 0,
    }
}

/// Embed claims and enumerate one hard-bounded deterministic pair page.
/// Returns `None` when fewer than two claims exist.
pub(super) fn prepare_pairs(
    claims: &[(ClaimId, ClaimView)],
    embedder: &dyn Embedder,
    thresholds: RelateThresholds,
    settled: &BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    temporal: &RelateTemporalPolicy,
    candidate_pair_budget: usize,
    candidate_cursor: CandidateCursor,
) -> Result<Option<PreparedPairs>, PipelineError> {
    if claims.len() < 2 {
        return Ok(None);
    }
    let texts: Vec<&str> = claims.iter().map(|(_, v)| embedding_text(v)).collect();
    let embeddings = embedder.embed_batch(&texts)?;

    // One deterministic global pair space makes both work and memory page
    // bounded. The pair prefilter remains the semantic floor; paging replaces
    // connected-component fan-out without weakening that explicit judge gate.
    let clusters = vec![(0..claims.len()).collect::<Vec<_>>()];
    let claim_clusters = vec![0_usize; claims.len()];
    let candidate_floor = thresholds.prefilter;
    let total_pairs = clusters
        .iter()
        .map(|cluster| pair_count(cluster.len()))
        .sum::<u64>();
    let start = candidate_cursor.offset().min(total_pairs);
    let end = start
        .saturating_add(u64::try_from(candidate_pair_budget).unwrap_or(u64::MAX))
        .min(total_pairs);
    let mut pending = Vec::new();
    let mut position = seek_pair(&clusters, start);
    let mut ordinal = start;
    while ordinal < end {
        let Some((cluster_idx, left_pos, right_pos)) = position else {
            break;
        };
        let cluster = &clusters[cluster_idx];
        let i = cluster[left_pos];
        let j = cluster[right_pos];
        if cosine_similarity(&embeddings[i], &embeddings[j]) >= candidate_floor {
            let (old_idx, new_idx, temporal_failure) = order_pair(claims, i, j, settled, temporal);
            if claims[old_idx].1.normalized_text != claims[new_idx].1.normalized_text {
                pending.push(PendingPair {
                    old_idx,
                    new_idx,
                    older: claims[old_idx].0.clone(),
                    newer: claims[new_idx].0.clone(),
                    temporal_failure,
                    cursor: CandidateCursor::from_offset(ordinal),
                });
            }
        }
        ordinal = ordinal.saturating_add(1);
        position = next_pair(&clusters, cluster_idx, left_pos, right_pos);
    }
    Ok(Some(PreparedPairs {
        pending,
        examined_pairs: usize::try_from(end.saturating_sub(start)).unwrap_or(usize::MAX),
        next_cursor: (end < total_pairs).then(|| CandidateCursor::from_offset(end)),
        claim_clusters,
    }))
}

fn pair_count(len: usize) -> u64 {
    let len = u64::try_from(len).unwrap_or(u64::MAX);
    len.saturating_mul(len.saturating_sub(1)) / 2
}

fn seek_pair(clusters: &[Vec<usize>], mut offset: u64) -> Option<(usize, usize, usize)> {
    for (cluster_idx, cluster) in clusters.iter().enumerate() {
        let count = pair_count(cluster.len());
        if offset >= count {
            offset -= count;
            continue;
        }
        for left in 0..cluster.len().saturating_sub(1) {
            let row = u64::try_from(cluster.len() - left - 1).unwrap_or(u64::MAX);
            if offset < row {
                let right = left + 1 + usize::try_from(offset).unwrap_or(usize::MAX);
                return Some((cluster_idx, left, right));
            }
            offset -= row;
        }
    }
    None
}

fn next_pair(
    clusters: &[Vec<usize>],
    cluster_idx: usize,
    left: usize,
    right: usize,
) -> Option<(usize, usize, usize)> {
    let cluster = &clusters[cluster_idx];
    if right + 1 < cluster.len() {
        return Some((cluster_idx, left, right + 1));
    }
    if left + 2 < cluster.len() {
        return Some((cluster_idx, left + 1, left + 2));
    }
    clusters
        .iter()
        .enumerate()
        .skip(cluster_idx + 1)
        .find(|(_, candidate)| candidate.len() >= 2)
        .map(|(next, _)| (next, 0, 1))
}

fn order_pair(
    claims: &[(ClaimId, ClaimView)],
    left: usize,
    right: usize,
    settled: &BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    temporal: &RelateTemporalPolicy,
) -> (usize, usize, Option<RelationFailureClass>) {
    let left_id = &claims[left].0;
    let right_id = &claims[right].0;
    if settled.contains_key(&(left_id.clone(), right_id.clone())) {
        return (left, right, None);
    }
    if settled.contains_key(&(right_id.clone(), left_id.clone())) {
        return (right, left, None);
    }
    let sequence_order = || {
        if (sequence_rank(&claims[left].1), left) <= (sequence_rank(&claims[right].1), right) {
            (left, right)
        } else {
            (right, left)
        }
    };
    match temporal.compare_claims(left_id, right_id) {
        None | Some(TemporalRelation::Same) => {
            let (old, new) = sequence_order();
            (old, new, None)
        }
        Some(TemporalRelation::Before) => (left, right, None),
        Some(TemporalRelation::After) => (right, left, None),
        Some(TemporalRelation::Concurrent) => {
            let (old, new) = sequence_order();
            (old, new, Some(RelationFailureClass::TemporalConcurrent))
        }
        Some(TemporalRelation::Unknown) => {
            let (old, new) = sequence_order();
            (old, new, Some(RelationFailureClass::TemporalUnknown))
        }
    }
}

/// Acquire one pair's verdict on the calling thread: journal authority first,
/// then the wall budget, then a live judge call.
pub(super) fn acquire_sequential(
    claims: &[(ClaimId, ClaimView)],
    pair: &PendingPair,
    relater: &dyn ClaimRelater,
    settled: &BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    started: Instant,
    budget: Duration,
) -> PairOutcome {
    let key = (pair.older.clone(), pair.newer.clone());
    if let Some(verdict) = settled.get(&key) {
        // DECISION(campaign): any journaled judgment settles the logical pair.
        // Explicit authority supersession is the future hook; model/config
        // changes never re-judge here.
        return PairOutcome::Judged(*verdict, true);
    }
    if let Some(class) = pair.temporal_failure {
        return PairOutcome::Failed(PairFailureView {
            class,
            endpoint: None,
            status: None,
            attempts: 0,
        });
    }
    if started.elapsed() >= budget {
        return PairOutcome::Failed(budget_exhausted());
    }
    // Feed raw claim text: case and update wording carry the intent signal
    // that normalized embedding text discards.
    let old_view = &claims[pair.old_idx].1;
    let new_view = &claims[pair.new_idx].1;
    match relater.relate(&old_view.text, &new_view.text) {
        Ok(verdict) => PairOutcome::Judged(verdict, false),
        Err(error) => PairOutcome::Failed(classify_pair_failure(&error)),
    }
}
