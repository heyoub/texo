use super::candidate::{PairOutcome, PendingPair};
use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::events::ids::{conflict_id_from_pair, ClaimId, ConflictId};
use crate::knowledge::TemporalRelation;
use crate::relate::settlement::{HeldDecision, UnresolvedPair};
use crate::semantics::ClaimRelation;

use super::{
    sequence_rank, CandidateCursor, ClaimView, CompleteRelateOutcome, ConflictEntry,
    ConflictStatus, PairJudgment, PartialRelateOutcome, RelateOutcome, RelateTemporalPolicy,
    RelatedClaims, SupersessionEdge,
};

/// Fold acquired outcomes into the deterministic decision reduction. Outcome
/// order equals pending order, so judgments/unresolved vectors — and therefore
/// journal append order — are byte-identical to the sequential path.
fn reduce_outcomes(
    claims: &[(ClaimId, ClaimView)],
    pending: Vec<PendingPair>,
    outcomes: Vec<PairOutcome>,
    temporal: &RelateTemporalPolicy,
) -> ReducedOutcome {
    let reduction = collect_pair_reduction(claims, pending, outcomes, temporal);
    let mut tainted = reduction
        .unresolved
        .iter()
        .flat_map(|pair| [pair.old_claim.clone(), pair.new_claim.clone()])
        .collect::<BTreeSet<_>>();
    tainted.extend(
        reduction
            .ambiguous_winners
            .into_iter()
            .map(|idx| claims[idx].0.clone()),
    );
    let (superseded, supersessions, mut held) =
        reduce_supersessions(claims, &reduction.winners, &tainted);
    let conflicts = reduce_conflicts(
        claims,
        reduction.conflict_pairs,
        &tainted,
        &superseded,
        &mut held,
    );
    ReducedOutcome {
        related: RelatedClaims {
            supersessions,
            conflicts,
            judgments: reduction.judgments,
        },
        unresolved: reduction.unresolved,
        held,
    }
}

struct ReducedOutcome {
    related: RelatedClaims,
    unresolved: Vec<UnresolvedPair>,
    held: Vec<HeldDecision>,
}

pub(super) struct PageCompletion<'a> {
    pub(super) settled: &'a BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    pub(super) claim_clusters: &'a [usize],
    pub(super) examined_pairs: usize,
    pub(super) candidate_pair_budget: usize,
    pub(super) next_cursor: Option<CandidateCursor>,
}

pub(super) fn finish_page(
    claims: &[(ClaimId, ClaimView)],
    pending: Vec<PendingPair>,
    outcomes: Vec<PairOutcome>,
    temporal: &RelateTemporalPolicy,
    page: &PageCompletion<'_>,
) -> RelateOutcome {
    let retry_cursor = pending
        .iter()
        .zip(&outcomes)
        .filter_map(|(pair, outcome)| {
            matches!(outcome, PairOutcome::Failed(_)).then_some(pair.cursor)
        })
        .min();
    let resume = retry_cursor.into_iter().chain(page.next_cursor).min();
    let page_reduction = reduce_outcomes(claims, pending, outcomes, temporal);
    if let Some(next_candidate_cursor) = resume {
        return RelateOutcome::Partial(partial_outcome(
            page_reduction,
            page.examined_pairs,
            page.candidate_pair_budget,
            next_candidate_cursor,
        ));
    }

    let fresh = page_reduction
        .related
        .judgments
        .iter()
        .filter(|judgment| !judgment.reused_authority)
        .map(|judgment| (judgment.older_claim.clone(), judgment.newer_claim.clone()))
        .collect::<BTreeSet<_>>();
    let mut authority = (*page.settled).clone();
    for judgment in &page_reduction.related.judgments {
        authority.insert(
            (judgment.older_claim.clone(), judgment.newer_claim.clone()),
            judgment.verdict,
        );
    }
    let (all_pending, all_outcomes) =
        authoritative_pairs(claims, &authority, &fresh, page.claim_clusters);
    let complete = reduce_outcomes(claims, all_pending, all_outcomes, temporal);
    RelateOutcome::Complete(CompleteRelateOutcome {
        candidate_pairs: complete.related.judgments.len(),
        candidate_pair_budget: page.candidate_pair_budget,
        related: complete.related,
        held: complete.held,
    })
}

fn partial_outcome(
    reduction: ReducedOutcome,
    candidate_pairs: usize,
    candidate_pair_budget: usize,
    next_candidate_cursor: CandidateCursor,
) -> PartialRelateOutcome {
    let ReducedOutcome {
        related,
        unresolved,
        mut held,
    } = reduction;
    held.extend(
        related
            .supersessions
            .into_iter()
            .map(
                |(old_claim, new_claim, reason)| HeldDecision::Supersession {
                    old_claim,
                    new_claim,
                    reason,
                },
            ),
    );
    held.extend(
        related
            .conflicts
            .into_iter()
            .map(|conflict| HeldDecision::Conflict {
                conflict_id: conflict.conflict_id,
                claim_a: conflict.claim_a,
                claim_b: conflict.claim_b,
                reason: conflict.reason,
            }),
    );
    PartialRelateOutcome {
        judgments: related.judgments,
        unresolved,
        held,
        candidate_pairs,
        candidate_pair_budget,
        next_candidate_cursor,
    }
}

fn authoritative_pairs(
    claims: &[(ClaimId, ClaimView)],
    authority: &BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    fresh: &BTreeSet<(ClaimId, ClaimId)>,
    claim_clusters: &[usize],
) -> (Vec<PendingPair>, Vec<PairOutcome>) {
    let by_id = claims
        .iter()
        .enumerate()
        .map(|(idx, (id, _))| (id.as_str(), idx))
        .collect::<BTreeMap<_, _>>();
    let mut rows = authority
        .iter()
        .filter_map(|((older, newer), verdict)| {
            let old_idx = *by_id.get(older.as_str())?;
            let new_idx = *by_id.get(newer.as_str())?;
            Some((
                (
                    claim_clusters[old_idx.min(new_idx)],
                    old_idx.min(new_idx),
                    old_idx.max(new_idx),
                ),
                PendingPair {
                    old_idx,
                    new_idx,
                    older: older.clone(),
                    newer: newer.clone(),
                    temporal_failure: None,
                    cursor: CandidateCursor::start(),
                },
                PairOutcome::Judged(*verdict, !fresh.contains(&(older.clone(), newer.clone()))),
            ))
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.0);
    rows.into_iter()
        .map(|(_, pair, outcome)| (pair, outcome))
        .unzip()
}

struct PairReduction {
    winners: BTreeMap<usize, usize>,
    ambiguous_winners: BTreeSet<usize>,
    conflict_pairs: Vec<(usize, usize)>,
    judgments: Vec<PairJudgment>,
    unresolved: Vec<UnresolvedPair>,
}

fn collect_pair_reduction(
    claims: &[(ClaimId, ClaimView)],
    pending: Vec<PendingPair>,
    outcomes: Vec<PairOutcome>,
    temporal: &RelateTemporalPolicy,
) -> PairReduction {
    let mut winners: BTreeMap<usize, usize> = BTreeMap::new();
    let mut ambiguous_winners = BTreeSet::new();
    let mut conflict_pairs: Vec<(usize, usize)> = Vec::new();
    let mut judgments = Vec::new();
    let mut unresolved = Vec::new();
    for (pair, outcome) in pending.into_iter().zip(outcomes) {
        let old_view = &claims[pair.old_idx].1;
        let new_view = &claims[pair.new_idx].1;
        let (verdict, reused_authority) = match outcome {
            PairOutcome::Failed(failure) => {
                unresolved.push(super::runtime::unresolved_pair(
                    pair.older, pair.newer, old_view, new_view, failure,
                ));
                continue;
            }
            PairOutcome::Judged(verdict, reused) => (verdict, reused),
        };
        judgments.push(PairJudgment {
            older_claim: pair.older,
            newer_claim: pair.newer,
            verdict,
            reused_authority,
        });
        match verdict.relation {
            ClaimRelation::Supersedes => {
                let better = match winners.get(&pair.old_idx) {
                    None => true,
                    Some(&cur) => match compare_successors(claims, cur, pair.new_idx, temporal) {
                        SuccessorOrder::Candidate => true,
                        SuccessorOrder::Current => false,
                        SuccessorOrder::Ambiguous => {
                            ambiguous_winners.insert(pair.old_idx);
                            (sequence_rank(&claims[pair.new_idx].1), pair.new_idx)
                                > (sequence_rank(&claims[cur].1), cur)
                        }
                    },
                };
                if better {
                    winners.insert(pair.old_idx, pair.new_idx);
                }
            }
            ClaimRelation::Conflict => {
                conflict_pairs.push((
                    pair.old_idx.min(pair.new_idx),
                    pair.old_idx.max(pair.new_idx),
                ));
            }
            ClaimRelation::Duplicate | ClaimRelation::Unrelated => {}
        }
    }
    PairReduction {
        winners,
        ambiguous_winners,
        conflict_pairs,
        judgments,
        unresolved,
    }
}

fn reduce_supersessions(
    claims: &[(ClaimId, ClaimView)],
    winners: &BTreeMap<usize, usize>,
    tainted: &BTreeSet<ClaimId>,
) -> (HashSet<ClaimId>, Vec<SupersessionEdge>, Vec<HeldDecision>) {
    let mut held = Vec::new();
    let mut superseded = HashSet::new();
    let mut supersessions = Vec::new();
    for (&old, &new) in winners {
        let (old_id, _) = &claims[old];
        let (new_id, new_view) = &claims[new];
        let reason = format!(
            "superseded by {}:{}",
            new_view.source_path, new_view.line_start
        );
        if tainted.contains(old_id) {
            held.push(HeldDecision::Supersession {
                old_claim: old_id.clone(),
                new_claim: new_id.clone(),
                reason,
            });
        } else {
            superseded.insert(old_id.clone());
            supersessions.push((old_id.clone(), new_id.clone(), reason));
        }
    }
    supersessions.sort_by(|a, b| {
        a.0.as_str()
            .cmp(b.0.as_str())
            .then_with(|| a.1.as_str().cmp(b.1.as_str()))
    });
    (superseded, supersessions, held)
}

fn reduce_conflicts(
    claims: &[(ClaimId, ClaimView)],
    conflict_pairs: Vec<(usize, usize)>,
    tainted: &BTreeSet<ClaimId>,
    superseded: &HashSet<ClaimId>,
    held: &mut Vec<HeldDecision>,
) -> Vec<ConflictEntry> {
    let mut conflicts: Vec<ConflictEntry> = Vec::new();
    let mut seen: HashSet<ConflictId> = HashSet::new();
    for (i, j) in conflict_pairs {
        let (a_id, a_view) = &claims[i];
        let (b_id, b_view) = &claims[j];
        let conflict_id = conflict_id_from_pair(a_id, b_id);
        if !seen.insert(conflict_id.clone()) {
            continue;
        }
        let entry = ConflictEntry {
            conflict_id,
            claim_a: a_id.clone(),
            claim_b: b_id.clone(),
            subject_hint: a_view.subject_hint.clone(),
            reason: format!(
                "contradictory current claims: \"{}\" vs \"{}\"",
                a_view.text, b_view.text
            ),
            status: ConflictStatus::Open,
        };
        if tainted.contains(a_id) || tainted.contains(b_id) {
            held.push(HeldDecision::Conflict {
                conflict_id: entry.conflict_id,
                claim_a: entry.claim_a,
                claim_b: entry.claim_b,
                reason: entry.reason,
            });
        } else if !superseded.contains(a_id) && !superseded.contains(b_id) {
            conflicts.push(entry);
        }
    }
    conflicts.sort_by(|x, y| x.conflict_id.as_str().cmp(y.conflict_id.as_str()));
    conflicts
}

enum SuccessorOrder {
    Current,
    Candidate,
    Ambiguous,
}

fn compare_successors(
    claims: &[(ClaimId, ClaimView)],
    current: usize,
    candidate: usize,
    temporal: &RelateTemporalPolicy,
) -> SuccessorOrder {
    match temporal.compare_claims(&claims[current].0, &claims[candidate].0) {
        None | Some(TemporalRelation::Same) => {
            if (sequence_rank(&claims[candidate].1), candidate)
                > (sequence_rank(&claims[current].1), current)
            {
                SuccessorOrder::Candidate
            } else {
                SuccessorOrder::Current
            }
        }
        Some(TemporalRelation::Before) => SuccessorOrder::Candidate,
        Some(TemporalRelation::After) => SuccessorOrder::Current,
        Some(TemporalRelation::Concurrent | TemporalRelation::Unknown) => SuccessorOrder::Ambiguous,
    }
}
