//! Semantic supersession and conflict logic.
//!
//! This module replaces the exact `subject_hint` bucketing + replacement-keyword
//! supersession + brittle contradiction-signal pile (see [`crate::stale::check`]
//! and [`crate::conflicts::detect`]) with **meaning-based** logic driven by two
//! injected backends:
//!
//! * an [`Embedder`] — used for **cluster-first candidate generation**: claims
//!   are clustered into connected components over the cosine-similarity graph
//!   (see [`group_claims`]), and only *within-cluster* pairs that also pass a
//!   coarse cosine prefilter ever reach the judge, so obviously-unrelated claims
//!   never cost a judge call;
//! * a [`ClaimRelater`] — an LLM-as-judge that, for one candidate pair, makes the
//!   single richer call embeddings + 3-way NLI cannot: are the claims about the
//!   same subject, and does the newer one *update* the older (supersede) or merely
//!   *disagree* (conflict)? Measured against real models, a value replacement and
//!   a genuine disagreement are *both* mutual contradiction at the NLI level, and
//!   "Friday deploy" / "Friday release" embed almost identically — so neither
//!   embeddings nor NLI alone can separate them. [`relate_claims`] is that path.
//!
//! Every function here is **pure**: it takes the claims and the backends and
//! returns plain data, performing no I/O. The backends are trait objects so the
//! logic can be proven deterministically with in-test stubs (no model, no
//! network).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::{Duration, Instant};

use crate::events::ids::{conflict_id_from_pair, ClaimId, ConflictId, SourceId};
use crate::knowledge::{SourceSnapshotId, TemporalRelation};
use crate::relate::settlement::{
    HeldDecision, PairFailureView, RelationFailureClass, UnresolvedPair,
};
use crate::semantics::{cosine_similarity, ClaimRelater, ClaimRelation, Embedder, SemanticsError};

/// Active lifecycle status for a claim view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimStatus {
    /// Active non-superseded claim.
    Current,
    /// Replaced by a newer claim.
    Superseded,
    /// Participates in an open conflict.
    Conflicting,
    /// Status not yet determined.
    Unknown,
}

/// Current lifecycle status for a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictStatus {
    /// Conflict is open.
    Open,
    /// Conflict has been resolved.
    Resolved,
}

/// Local store sequence carried by a receipt view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalSequence(u64);

impl LocalSequence {
    /// Construct a local sequence wrapper.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the wrapped sequence.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Append receipt metadata required by the semantic pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptView {
    /// Local store commit sequence.
    pub sequence: LocalSequence,
}

/// Build a receipt view from bare append metadata.
#[must_use]
pub const fn receipt_view(
    _event_id: u128,
    sequence: u64,
    _kind: &str,
    _scope: &str,
    _entity: &str,
) -> ReceiptView {
    ReceiptView {
        sequence: LocalSequence::new(sequence),
    }
}

/// Replay claim projection consumed by the pure semantic pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimView {
    /// Claim id.
    pub claim_id: ClaimId,
    /// Workspace id.
    pub workspace_id: String,
    /// Source id.
    pub source_id: SourceId,
    /// Source path.
    pub source_path: String,
    /// Start line.
    pub line_start: u32,
    /// End line.
    pub line_end: u32,
    /// Raw text.
    pub text: String,
    /// Normalized text.
    pub normalized_text: String,
    /// Subject hint.
    pub subject_hint: String,
    /// Predicate hint.
    pub predicate_hint: String,
    /// Object hint.
    pub object_hint: String,
    /// Confidence ppm.
    pub confidence_ppm: u32,
    /// Extractor kind.
    pub extractor_kind: String,
    /// Lifecycle status.
    pub status: ClaimStatus,
    /// Receipt for claim recorded event.
    pub receipt: ReceiptView,
    /// Claim ids this claim supersedes (as new claim).
    pub supersedes: Vec<ClaimId>,
    /// If superseded, the replacing claim id.
    pub superseded_by: Option<ClaimId>,
}

/// Conflict entry derived by the semantic pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictEntry {
    /// Conflict id.
    pub conflict_id: ConflictId,
    /// First claim.
    pub claim_a: ClaimId,
    /// Second claim.
    pub claim_b: ClaimId,
    /// Subject hint shared by both claims.
    pub subject_hint: String,
    /// Heuristic reason.
    pub reason: String,
    /// Current status.
    pub status: ConflictStatus,
}

/// Failure raised while running the semantic pipeline.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// A backend ([`Embedder`] or [`ClaimRelater`]) failed.
    #[error("semantic backend failure")]
    Semantics(#[from] SemanticsError),
}

/// A supersession edge: `(old_claim, new_claim, reason)`.
///
/// Mirrors the tuple shape returned by
/// [`crate::stale::check::infer_supersessions`] so the two can be swapped.
pub type SupersessionEdge = (ClaimId, ClaimId, String);

/// The embedding text used for grouping a claim.
///
/// Prefers the normalized text (stable, lower-noise); falls back to the raw text
/// when normalization produced an empty string.
fn embedding_text(view: &ClaimView) -> &str {
    if view.normalized_text.is_empty() {
        &view.text
    } else {
        &view.normalized_text
    }
}

/// Cluster claims into subject groups by embedding cosine similarity.
///
/// Each claim's [`embedding_text`] is embedded once. Two claims are linked when
/// their cosine similarity is `>= threshold`; groups are the connected components
/// of that link graph (transitive — if A links B and B links C they share a
/// group even if A and C fall just under the threshold). This replaces exact
/// `subject_hint` bucketing with meaning-based clustering.
///
/// Returns groups as vectors of indices into `claims`. Indices within a group and
/// the groups themselves are ordered by ascending first member index, so the
/// result is deterministic for a given input order.
///
/// # Errors
///
/// Returns [`PipelineError::Semantics`] when the [`Embedder`] fails to embed the
/// claim texts.
pub fn group_claims(
    claims: &[(ClaimId, ClaimView)],
    embedder: &dyn Embedder,
    threshold: f32,
) -> Result<Vec<Vec<usize>>, PipelineError> {
    if claims.is_empty() {
        return Ok(Vec::new());
    }
    let texts: Vec<&str> = claims.iter().map(|(_, v)| embedding_text(v)).collect();
    let embeddings = embedder.embed_batch(&texts)?;
    Ok(similarity_components(&embeddings, threshold))
}

/// Connected components of the cosine-similarity link graph over `embeddings`.
///
/// Two indices are linked when their cosine similarity is `>= threshold`; the
/// components of that graph are returned as vectors of indices. The result is
/// **deterministic for a given input order**: it depends only on slice order
/// (never on hash-map iteration), members within a component are ascending, and
/// components are ordered by their first (smallest) member.
fn similarity_components(embeddings: &[Vec<f32>], threshold: f32) -> Vec<Vec<usize>> {
    let n = embeddings.len();

    // Union-find over indices.
    let mut parent: Vec<usize> = (0..n).collect();
    for i in 0..n {
        for j in (i + 1)..n {
            if cosine_similarity(&embeddings[i], &embeddings[j]) >= threshold {
                let ri = union_find_root(&mut parent, i);
                let rj = union_find_root(&mut parent, j);
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }

    // Bucket indices by their representative, preserving ascending order.
    let mut roots: Vec<usize> = Vec::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in 0..n {
        let r = union_find_root(&mut parent, i);
        if let Some(pos) = roots.iter().position(|&x| x == r) {
            groups[pos].push(i);
        } else {
            roots.push(r);
            groups.push(vec![i]);
        }
    }
    groups
}

/// Path-compressing union-find root lookup over a `parent` slice.
fn union_find_root(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

/// Sequence rank used to order claims oldest-to-newest within a group.
fn sequence_rank(view: &ClaimView) -> u64 {
    view.receipt.sequence.get()
}

/// Thresholds governing candidate generation in [`relate_claims`].
///
/// Both are in-memory cosine similarities (never journaled, so floats are fine
/// here — determinism of the *recorded* output comes from the record-once event
/// boundary, not from these gates).
#[derive(Debug, Clone, Copy)]
pub struct RelateThresholds {
    /// Link threshold for connected-component **clustering** (candidate
    /// generation). Pairs split across clusters are never judged; see
    /// [`relate_claims`]. Typically the `[semantics]` `cosine_threshold`.
    pub cluster: f32,
    /// Coarse per-pair cosine **prefilter** applied within a cluster. Must sit
    /// below the lowest same-subject similarity in the corpus (the relater does
    /// the real separating), so it is intentionally lower than `cluster`.
    pub prefilter: f32,
}

/// Frozen source-order evidence used to orient semantic claim pairs.
///
/// Claims without Git evidence retain journal observation order. Once either
/// claim is snapshot-backed, missing or incomparable ancestry is explicit and
/// blocks a live judgment instead of guessing from ingest order.
#[derive(Debug, Clone, Default)]
pub struct RelateTemporalPolicy {
    claim_snapshots: BTreeMap<String, String>,
    snapshot_relations: BTreeMap<(String, String), TemporalRelation>,
}

impl RelateTemporalPolicy {
    /// Bind a semantic claim to the frozen source snapshot containing its
    /// latest accepted evidence occurrence.
    pub fn bind_claim(&mut self, claim_id: &ClaimId, snapshot_id: &SourceSnapshotId) {
        self.claim_snapshots
            .insert(claim_id.to_string(), snapshot_id.to_string());
    }

    /// Add one replayed directed source-order fact.
    pub fn insert_relation(
        &mut self,
        left: &SourceSnapshotId,
        right: &SourceSnapshotId,
        relation: TemporalRelation,
    ) {
        self.snapshot_relations
            .entry((left.to_string(), right.to_string()))
            .or_insert(relation);
    }

    /// Add one replayed directed source-order fact by its already-validated
    /// durable identifiers.
    pub(crate) fn insert_relation_ids(
        &mut self,
        left: &str,
        right: &str,
        relation: TemporalRelation,
    ) {
        self.snapshot_relations
            .entry((left.to_string(), right.to_string()))
            .or_insert(relation);
    }

    fn compare_claims(&self, left: &ClaimId, right: &ClaimId) -> Option<TemporalRelation> {
        let left = self.claim_snapshots.get(left.as_str());
        let right = self.claim_snapshots.get(right.as_str());
        match (left, right) {
            (None, None) => None,
            (Some(left), Some(right)) if left == right => Some(TemporalRelation::Same),
            (Some(left), Some(right)) => Some(
                self.snapshot_relations
                    .get(&(left.clone(), right.clone()))
                    .copied()
                    .or_else(|| {
                        self.snapshot_relations
                            .get(&(right.clone(), left.clone()))
                            .copied()
                            .map(invert_temporal_relation)
                    })
                    .unwrap_or(TemporalRelation::Unknown),
            ),
            (Some(_), None) | (None, Some(_)) => Some(TemporalRelation::Unknown),
        }
    }
}

const fn invert_temporal_relation(relation: TemporalRelation) -> TemporalRelation {
    match relation {
        TemporalRelation::Before => TemporalRelation::After,
        TemporalRelation::After => TemporalRelation::Before,
        TemporalRelation::Same => TemporalRelation::Same,
        TemporalRelation::Concurrent => TemporalRelation::Concurrent,
        TemporalRelation::Unknown => TemporalRelation::Unknown,
    }
}

/// Both relations the semantic pipeline derives, in a single pass.
#[derive(Debug, Default)]
pub struct RelatedClaims {
    /// Supersession edges `(old, new, reason)`; each superseded claim appears once,
    /// linked to the newest claim that supersedes it.
    pub supersessions: Vec<SupersessionEdge>,
    /// Open conflicts between contradictory claims that are *both* still current
    /// (neither has been superseded).
    pub conflicts: Vec<ConflictEntry>,
    /// Successful verdicts for every judged pair, including unrelated pairs.
    pub judgments: Vec<PairJudgment>,
}

/// One successful logical-pair judgment.
#[derive(Debug, Clone, PartialEq)]
pub struct PairJudgment {
    /// Older claim.
    pub older_claim: ClaimId,
    /// Newer claim.
    pub newer_claim: ClaimId,
    /// Model verdict.
    pub verdict: crate::semantics::RelationVerdict,
    /// True when authority was loaded from the journal instead of re-judged.
    pub reused_authority: bool,
}

/// Complete, partial, or fully held semantic pipeline result.
#[derive(Debug, Default)]
pub struct RelateOutcome {
    /// Decisions whose full evidence set is present.
    pub related: RelatedClaims,
    /// Candidate pairs for which no verdict exists.
    pub unresolved: Vec<UnresolvedPair>,
    /// Decisions withheld by the tainted-claim holdback rule.
    pub held: Vec<HeldDecision>,
}

impl std::ops::Deref for RelateOutcome {
    type Target = RelatedClaims;

    fn deref(&self) -> &Self::Target {
        &self.related
    }
}

/// Relate claims by a single richer judgment per candidate pair.
///
/// This is the primary relating entry point. A 3-way NLI label cannot distinguish
/// a value replacement from a genuine disagreement — measured against real models,
/// *both* are mutual contradiction, and embeddings alone cannot tell "Friday
/// deploy" from "Friday release". A [`ClaimRelater`] answers both questions
/// (shared subject? update or conflict?) at once.
///
/// Candidate generation is **cluster-first** so the judge-call count scales with
/// cluster sizes, not with the corpus:
/// 1. Embed every claim once; cluster the claims into connected components of the
///    cosine-similarity graph at [`RelateThresholds::cluster`] (the same
///    clustering as [`group_claims`]).
/// 2. Within each cluster, consider only pairs whose cosine similarity is
///    `>= prefilter` — a *coarse* recall gate that should sit **below** the
///    lowest same-subject similarity in the corpus, never high enough to do the
///    separating itself (that is the relater's job).
/// 3. Order each surviving pair oldest→newest (by `receipt.sequence`, index as a
///    deterministic tiebreak) and ask the relater how the newer relates to the
///    older. Identical-normalized-text pairs are skipped as duplicates.
/// 4. [`ClaimRelation::Supersedes`] → the older is superseded; among all claims
///    that supersede it, the **newest** wins (one canonical edge per stale claim).
/// 5. [`ClaimRelation::Conflict`] → a candidate conflict, kept only if **neither**
///    side was superseded in step 4 (a superseded claim is no longer current).
///
/// # Cross-cluster pairs are deliberately skipped
///
/// A pair whose claims land in different clusters is **never judged**, even when
/// its cosine similarity clears the prefilter — that skip is the point: it bounds
/// judge calls to `Σ (|cluster| choose 2)` (roughly `O(n · max_cluster)`) instead
/// of `O(n²)` over the whole corpus, which is what makes relate practical on
/// large corpora. Same-subject claims embed well above any sane cluster
/// threshold (and components link transitively), so a genuinely related pair
/// landing in two clusters means the cluster threshold is set above the corpus's
/// same-subject similarity floor — lower `[semantics]` `cosine_threshold` rather
/// than the prefilter. With `cluster <= prefilter` the judged pair set is
/// identical to the pre-clustering behavior (every pair passing the prefilter is
/// by definition intra-cluster). For any pair that *is* judged, the verdict and
/// event semantics are exactly those of the pre-clustering pipeline.
///
/// Pure and deterministic for a given input order and backend behavior:
/// clustering, pair enumeration, and all output ordering depend only on slice
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

/// A candidate pair that survived clustering, the prefilter, and the
/// duplicate-text check, ordered oldest -> newest.
struct PendingPair {
    old_idx: usize,
    new_idx: usize,
    older: ClaimId,
    newer: ClaimId,
    temporal_failure: Option<RelationFailureClass>,
}

/// Verdict-acquisition result for one pending pair. Clone so a coalesced text
/// pair's single outcome fans to every logical pair that shares its texts.
#[derive(Clone)]
enum PairOutcome {
    Judged(crate::semantics::RelationVerdict, bool),
    Failed(PairFailureView),
}

/// The failure view for a pair the wall budget cut off before it ran.
fn budget_exhausted() -> PairFailureView {
    PairFailureView {
        class: RelationFailureClass::BudgetExhausted,
        endpoint: None,
        status: None,
        attempts: 0,
    }
}

/// Embed, cluster, and enumerate surviving candidate pairs in deterministic
/// order. Returns `None` when fewer than two claims exist.
fn prepare_pairs(
    claims: &[(ClaimId, ClaimView)],
    embedder: &dyn Embedder,
    thresholds: RelateThresholds,
    settled: &BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    temporal: &RelateTemporalPolicy,
) -> Result<Option<Vec<PendingPair>>, PipelineError> {
    if claims.len() < 2 {
        return Ok(None);
    }
    let texts: Vec<&str> = claims.iter().map(|(_, v)| embedding_text(v)).collect();
    let embeddings = embedder.embed_batch(&texts)?;

    // Cluster first: the judge only ever sees within-cluster pairs.
    let clusters = similarity_components(&embeddings, thresholds.cluster);
    let mut pending = Vec::new();
    for cluster in &clusters {
        for (pos, &i) in cluster.iter().enumerate() {
            for &j in &cluster[pos + 1..] {
                // Cluster members are ascending, so i < j always holds here.
                if cosine_similarity(&embeddings[i], &embeddings[j]) < thresholds.prefilter {
                    continue;
                }
                let (old_idx, new_idx, temporal_failure) =
                    order_pair(claims, i, j, settled, temporal);
                if claims[old_idx].1.normalized_text == claims[new_idx].1.normalized_text {
                    continue;
                }
                pending.push(PendingPair {
                    old_idx,
                    new_idx,
                    older: claims[old_idx].0.clone(),
                    newer: claims[new_idx].0.clone(),
                    temporal_failure,
                });
            }
        }
    }
    Ok(Some(pending))
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
fn acquire_sequential(
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

/// Fold acquired outcomes into the deterministic decision reduction. Outcome
/// order equals pending order, so judgments/unresolved vectors — and therefore
/// journal append order — are byte-identical to the sequential path.
#[expect(
    clippy::too_many_lines,
    reason = "holdback and deterministic decision reduction form one state-machine fold"
)]
fn reduce_outcomes(
    claims: &[(ClaimId, ClaimView)],
    pending: Vec<PendingPair>,
    outcomes: Vec<PairOutcome>,
    temporal: &RelateTemporalPolicy,
) -> RelateOutcome {
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
                unresolved.push(unresolved_pair(
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

    let mut tainted = unresolved
        .iter()
        .flat_map(|pair| [pair.old_claim.clone(), pair.new_claim.clone()])
        .collect::<BTreeSet<_>>();
    tainted.extend(
        ambiguous_winners
            .into_iter()
            .map(|idx| claims[idx].0.clone()),
    );
    let mut held = Vec::new();
    let mut superseded = HashSet::new();
    let mut supersessions = Vec::new();
    for (&old, &new) in &winners {
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

    RelateOutcome {
        related: RelatedClaims {
            supersessions,
            conflicts,
            judgments,
        },
        unresolved,
        held,
    }
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
    let Some(pending) = prepare_pairs(claims, embedder, thresholds, settled, temporal)? else {
        return Ok(RelateOutcome::default());
    };
    let outcomes = pending
        .iter()
        .map(|pair| acquire_sequential(claims, pair, relater, settled, started, budget))
        .collect::<Vec<_>>();
    Ok(reduce_outcomes(claims, pending, outcomes, temporal))
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
    } = options;
    let started = Instant::now();
    let Some(pending) = prepare_pairs(claims, embedder, thresholds, settled, temporal)? else {
        return Ok(RelateOutcome::default());
    };
    if concurrency <= 1 {
        let outcomes = pending
            .iter()
            .map(|pair| acquire_sequential(claims, pair, relater, settled, started, budget))
            .collect::<Vec<_>>();
        return Ok(reduce_outcomes(claims, pending, outcomes, temporal));
    }

    let mut outcomes: Vec<Option<PairOutcome>> = Vec::with_capacity(pending.len());
    outcomes.resize_with(pending.len(), || None);

    // Coalesce unsettled jobs by their exact relate() arguments. The disk cache
    // keys on (fingerprint, older_text, newer_text), so two logical pairs with
    // identical texts MUST make one model call and share the verdict: otherwise
    // a cold parallel run makes duplicate paid calls, can receive different
    // verdicts for identical prompts, and races writes to one cache tmp file.
    // The representative is the lowest pending index in each group, so the
    // choice is deterministic. Settled pairs resolve here and never dispatch.
    let mut groups: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    for (idx, pair) in pending.iter().enumerate() {
        if let Some(verdict) = settled.get(&(pair.older.clone(), pair.newer.clone())) {
            outcomes[idx] = Some(PairOutcome::Judged(*verdict, true));
            continue;
        }
        if let Some(class) = pair.temporal_failure {
            outcomes[idx] = Some(PairOutcome::Failed(PairFailureView {
                class,
                endpoint: None,
                status: None,
                attempts: 0,
            }));
            continue;
        }
        let texts = (
            claims[pair.old_idx].1.text.as_str(),
            claims[pair.new_idx].1.text.as_str(),
        );
        groups.entry(texts).or_default().push(idx);
    }
    let mut representatives: Vec<usize> = groups.values().map(|members| members[0]).collect();
    // `BTreeMap` orders groups by text, but budget priority is part of the
    // deterministic pair-enumeration contract. Restore pending-pair order
    // after coalescing so a tight wall budget considers the same earliest
    // logical pairs as the sequential path.
    representatives.sort_unstable();

    let mut rep_outcomes: BTreeMap<usize, PairOutcome> = std::thread::scope(|scope| {
        let (job_tx, job_rx) = flume::bounded::<usize>(concurrency);
        let (result_tx, result_rx) = flume::unbounded::<(usize, PairOutcome)>();
        for _ in 0..concurrency {
            let job_rx = job_rx.clone();
            let result_tx = result_tx.clone();
            let pending = &pending;
            scope.spawn(move || {
                while let Ok(idx) = job_rx.recv() {
                    let pair = &pending[idx];
                    let outcome = if started.elapsed() >= budget {
                        PairOutcome::Failed(budget_exhausted())
                    } else {
                        let old_view = &claims[pair.old_idx].1;
                        let new_view = &claims[pair.new_idx].1;
                        match relater.relate(&old_view.text, &new_view.text) {
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
        for idx in &representatives {
            if job_tx.send(*idx).is_err() {
                break;
            }
        }
        drop(job_tx);
        result_rx.iter().collect()
    });

    // Fan each representative's single outcome to every member of its group.
    for members in groups.values() {
        let outcome = rep_outcomes
            .remove(&members[0])
            .unwrap_or(PairOutcome::Failed(budget_exhausted()));
        for &idx in members {
            outcomes[idx] = Some(outcome.clone());
        }
    }

    let outcomes = outcomes
        .into_iter()
        .map(|slot| slot.unwrap_or(PairOutcome::Failed(budget_exhausted())))
        .collect::<Vec<_>>();
    Ok(reduce_outcomes(claims, pending, outcomes, temporal))
}

fn unresolved_pair(
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::extract::normalize::normalize_line;

    /// Deterministic embedder driven by a fixed text -> vector table.
    ///
    /// Lookup is by the first table entry whose key is a case-insensitive
    /// substring of the embedded text, so callers key on a distinctive phrase
    /// from each claim. Texts with no matching key get a unique orthogonal basis
    /// vector (never grouped with anything), making "unmapped" inputs inert
    /// rather than accidentally similar.
    struct FixedEmbedder {
        table: Vec<(&'static str, Vec<f32>)>,
        width: usize,
    }

    impl FixedEmbedder {
        fn new(table: Vec<(&'static str, Vec<f32>)>, width: usize) -> Self {
            Self { table, width }
        }

        /// One-hot vector for an unmapped text, derived from its byte sum so the
        /// same text is stable but distinct texts rarely collide.
        fn fallback(&self, text: &str) -> Vec<f32> {
            let mut out = vec![0.0f32; self.width];
            let sum: usize = text.bytes().map(usize::from).sum();
            out[sum % self.width] = 1.0;
            out
        }
    }

    impl Embedder for FixedEmbedder {
        fn embed(&self, text: &str) -> Result<Vec<f32>, SemanticsError> {
            let lower = text.to_ascii_lowercase();
            for (key, vector) in &self.table {
                if lower.contains(&key.to_ascii_lowercase()) {
                    return Ok(vector.clone());
                }
            }
            Ok(self.fallback(text))
        }
    }

    use crate::semantics::RelationVerdict;

    /// Deterministic relater driven by an `(older_sub, newer_sub) -> relation`
    /// table. The first entry whose substrings match both the older premise and
    /// the newer hypothesis wins; unmatched pairs are [`ClaimRelation::Unrelated`]
    /// (the safe default — no edge, no conflict). Keyed on distinctive phrases.
    struct ScriptedRelater {
        table: Vec<(&'static str, &'static str, ClaimRelation)>,
    }

    impl ScriptedRelater {
        fn new(table: Vec<(&'static str, &'static str, ClaimRelation)>) -> Self {
            Self { table }
        }
    }

    impl ClaimRelater for ScriptedRelater {
        fn relate(&self, older: &str, newer: &str) -> Result<RelationVerdict, SemanticsError> {
            let o = older.to_ascii_lowercase();
            let nw = newer.to_ascii_lowercase();
            for (older_sub, newer_sub, relation) in &self.table {
                if o.contains(&older_sub.to_ascii_lowercase())
                    && nw.contains(&newer_sub.to_ascii_lowercase())
                {
                    return Ok(RelationVerdict {
                        relation: *relation,
                        score: 1.0,
                    });
                }
            }
            Ok(RelationVerdict {
                relation: ClaimRelation::Unrelated,
                score: 1.0,
            })
        }
        fn fingerprint(&self) -> String {
            "scripted".to_owned()
        }
    }

    /// Shorthand for [`RelateThresholds`]. Passing `cluster == prefilter`
    /// reproduces the pre-clustering judged pair set exactly (every pair passing
    /// the prefilter is intra-cluster by definition), which is what the original
    /// single-threshold tests exercised.
    fn th(cluster: f32, prefilter: f32) -> RelateThresholds {
        RelateThresholds { cluster, prefilter }
    }

    fn claim(id: &str, subject: &str, text: &str, sequence: u64) -> (ClaimId, ClaimView) {
        let claim_id = ClaimId::try_from(id).expect("valid claim id");
        let view = ClaimView {
            claim_id: claim_id.clone(),
            workspace_id: "demo".to_string(),
            source_id: SourceId::try_from("src_abc123def456").expect("valid source id"),
            source_path: "x.md".to_string(),
            line_start: u32::try_from(sequence).unwrap_or(u32::MAX),
            line_end: u32::try_from(sequence).unwrap_or(u32::MAX),
            text: text.to_string(),
            normalized_text: normalize_line(text),
            subject_hint: subject.to_string(),
            predicate_hint: "unknown".to_string(),
            object_hint: text.to_ascii_lowercase(),
            confidence_ppm: 650_000,
            extractor_kind: "test".to_string(),
            status: ClaimStatus::Current,
            receipt: receipt_view(
                sequence.into(),
                sequence,
                "ClaimRecorded",
                "workspace:demo",
                id,
            ),
            supersedes: Vec::new(),
            superseded_by: None,
        };
        (claim_id, view)
    }

    /// Build the embedder for the deploy-schedule scenario: the three deploy-day
    /// claims plus the noise claim all sit in the same cluster (they are about the
    /// deploy day), so grouping is purely about meaning while supersession is left
    /// to NLI to decide.
    fn deploy_embedder() -> FixedEmbedder {
        FixedEmbedder::new(
            vec![
                ("friday", vec![1.0, 0.0, 0.0]),
                ("wednesday", vec![0.98, 0.10, 0.0]),
                ("tuesday", vec![0.97, 0.12, 0.0]),
                ("asked about the deploy day", vec![0.96, 0.14, 0.0]),
            ],
            3,
        )
    }

    #[test]
    fn deploy_schedule_groups_three_days_and_noise_together() {
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "Deploys happen on Friday", 1),
            claim("claim_bbbbbbbbbbbb", "x", "Deploys moved to Wednesday", 2),
            claim("claim_cccccccccccc", "x", "Deploys moved to Tuesday", 3),
            claim(
                "claim_dddddddddddd",
                "x",
                "dave asked about the deploy day",
                2,
            ),
        ];
        let groups = group_claims(&claims, &deploy_embedder(), 0.9).expect("group");
        assert_eq!(groups.len(), 1, "all four cluster on deploy-day meaning");
        assert_eq!(groups[0].len(), 4);
    }

    /// Embedder for the release scenario: the two release-schedule claims cluster
    /// together, but "Bob owns release approval" is a DIFFERENT subject and must
    /// land in its own group (the key dogfood trap — same word, different
    /// meaning).
    fn release_embedder() -> FixedEmbedder {
        FixedEmbedder::new(
            vec![
                ("releases happen on monday", vec![1.0, 0.0]),
                ("go out on friday", vec![0.95, 0.05]),
                ("bob owns release approval", vec![0.0, 1.0]),
            ],
            2,
        )
    }

    #[test]
    fn release_schedule_splits_from_release_approval_by_meaning() {
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "Releases happen on Monday", 1),
            claim("claim_bbbbbbbbbbbb", "x", "Releases go out on Friday", 2),
            claim("claim_cccccccccccc", "x", "Bob owns release approval", 3),
        ];
        let groups = group_claims(&claims, &release_embedder(), 0.9).expect("group");
        assert_eq!(
            groups.len(),
            2,
            "schedule and approval are different subjects"
        );
        // The schedule pair groups together; approval is alone.
        let sizes: Vec<usize> = {
            let mut s: Vec<usize> = groups.iter().map(Vec::len).collect();
            s.sort_unstable();
            s
        };
        assert_eq!(sizes, vec![1, 2]);
    }

    #[test]
    fn backend_error_propagates() {
        struct FailingEmbedder;
        impl Embedder for FailingEmbedder {
            fn embed(&self, _text: &str) -> Result<Vec<f32>, SemanticsError> {
                Err(SemanticsError::DimensionMismatch {
                    expected: 2,
                    actual: 1,
                })
            }
        }
        let claims = vec![claim("claim_aaaaaaaaaaaa", "x", "anything", 1)];
        let err = group_claims(&claims, &FailingEmbedder, 0.9).expect_err("must propagate");
        assert!(matches!(err, PipelineError::Semantics(_)));
    }

    #[test]
    fn group_claims_empty_input_is_empty() {
        let embedder = FixedEmbedder::new(Vec::new(), 2);
        assert!(group_claims(&[], &embedder, 0.9).expect("group").is_empty());
    }

    #[test]
    fn grouping_is_transitive_via_connected_components() {
        // A links B, B links C, but A does not directly link C; connected
        // components still place all three in one group.
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "alpha", 1),
            claim("claim_bbbbbbbbbbbb", "x", "bravo", 2),
            claim("claim_cccccccccccc", "x", "charlie", 3),
        ];
        let embedder = FixedEmbedder::new(
            vec![
                ("alpha", vec![1.0, 0.0]),
                ("bravo", vec![0.95, 0.31]),
                ("charlie", vec![0.80, 0.60]),
            ],
            2,
        );
        // alpha-bravo cosine ~0.95 (>=0.9), bravo-charlie ~0.95 (>=0.9), but
        // alpha-charlie ~0.80 (<0.9): only connected components unite all three.
        let groups = group_claims(&claims, &embedder, 0.9).expect("group");
        assert_eq!(groups.len(), 1, "transitive chain forms one component");
        assert_eq!(groups[0].len(), 3);
    }

    // --- relate_claims (the LLM-relation-judge path) ---

    #[test]
    fn relate_supersession_chain_picks_newest_winner_and_ignores_noise() {
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "Deploys happen on Friday", 1),
            claim("claim_bbbbbbbbbbbb", "x", "Deploys moved to Wednesday", 2),
            claim("claim_cccccccccccc", "x", "Deploys moved to Tuesday", 3),
            claim(
                "claim_dddddddddddd",
                "x",
                "dave asked about the deploy day",
                2,
            ),
        ];
        // The judge reports each newer deploy decision as superseding the older;
        // the noise question is unrelated to every deploy claim.
        let relater = ScriptedRelater::new(vec![
            ("friday", "wednesday", ClaimRelation::Supersedes),
            ("friday", "tuesday", ClaimRelation::Supersedes),
            ("wednesday", "tuesday", ClaimRelation::Supersedes),
        ]);
        let out =
            relate_claims(&claims, &deploy_embedder(), &relater, th(0.9, 0.9)).expect("relate");

        // Friday and Wednesday each superseded by Tuesday (the newest winner).
        assert_eq!(out.supersessions.len(), 2);
        let pairs: Vec<(&str, &str)> = out
            .supersessions
            .iter()
            .map(|(o, n, _)| (o.as_str(), n.as_str()))
            .collect();
        assert!(pairs.contains(&("claim_aaaaaaaaaaaa", "claim_cccccccccccc")));
        assert!(pairs.contains(&("claim_bbbbbbbbbbbb", "claim_cccccccccccc")));
        assert!(
            !pairs
                .iter()
                .any(|(o, n)| *o == "claim_dddddddddddd" || *n == "claim_dddddddddddd"),
            "noise never participates in supersession"
        );
        assert!(out.conflicts.is_empty(), "no conflicts in a clean chain");
    }

    #[test]
    fn relate_release_disagreement_is_conflict_not_supersession() {
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "Releases happen on Monday", 1),
            claim("claim_bbbbbbbbbbbb", "x", "Releases go out on Friday", 2),
            claim("claim_cccccccccccc", "x", "Bob owns release approval", 3),
        ];
        // Monday vs Friday disagree with no update intent -> conflict. Approval is
        // a different subject and never grouped with the schedule pair.
        let relater = ScriptedRelater::new(vec![("monday", "friday", ClaimRelation::Conflict)]);
        let out =
            relate_claims(&claims, &release_embedder(), &relater, th(0.9, 0.9)).expect("relate");

        assert!(
            out.supersessions.is_empty(),
            "a flat disagreement is not a supersession"
        );
        assert_eq!(out.conflicts.len(), 1, "exactly one release conflict");
        let entry = &out.conflicts[0];
        let mut pair = [entry.claim_a.as_str(), entry.claim_b.as_str()];
        pair.sort_unstable();
        assert_eq!(pair, ["claim_aaaaaaaaaaaa", "claim_bbbbbbbbbbbb"]);
        assert_eq!(entry.status, ConflictStatus::Open);
    }

    #[test]
    fn relate_superseded_claim_cannot_also_conflict() {
        // A claim that is superseded must not surface as a live conflict, even if
        // the judge also reports a contradicting peer.
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "Deploys happen on Friday", 1),
            claim("claim_bbbbbbbbbbbb", "x", "Deploys moved to Tuesday", 3),
            claim("claim_cccccccccccc", "x", "Deploys happen on Monday", 2),
        ];
        let relater = ScriptedRelater::new(vec![
            ("friday", "tuesday", ClaimRelation::Supersedes),
            ("monday", "tuesday", ClaimRelation::Supersedes),
            ("friday", "monday", ClaimRelation::Conflict),
        ]);
        // All three deploy-day claims must cluster (deploy_embedder omits Monday).
        let embedder = FixedEmbedder::new(
            vec![
                ("friday", vec![1.0, 0.0, 0.0]),
                ("tuesday", vec![0.97, 0.12, 0.0]),
                ("monday", vec![0.96, 0.14, 0.0]),
            ],
            3,
        );
        let out = relate_claims(&claims, &embedder, &relater, th(0.9, 0.9)).expect("relate");
        // Friday and Monday are both superseded by Tuesday, so the Friday/Monday
        // conflict is dropped.
        assert_eq!(out.supersessions.len(), 2);
        assert!(
            out.conflicts.is_empty(),
            "conflict involving a superseded claim is dropped"
        );
    }

    #[test]
    fn relate_duplicate_text_is_skipped() {
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "Deploys moved to Tuesday", 1),
            claim("claim_bbbbbbbbbbbb", "x", "Deploys moved to Tuesday", 2),
        ];
        let embedder = FixedEmbedder::new(vec![("tuesday", vec![1.0, 0.0])], 2);
        // Even if the judge would fire, identical normalized text is never judged.
        let relater = ScriptedRelater::new(vec![("tuesday", "tuesday", ClaimRelation::Supersedes)]);
        let out = relate_claims(&claims, &embedder, &relater, th(0.9, 0.9)).expect("relate");
        assert!(out.supersessions.is_empty());
        assert!(out.conflicts.is_empty());
    }

    /// A relater that panics if ever called: proves a candidate-generation gate
    /// (cluster split or prefilter) drops a pair before any judge call.
    struct NeverRelater;
    impl ClaimRelater for NeverRelater {
        #[expect(
            clippy::panic,
            reason = "test guard: reaching the judge for a gated-out pair is the failure being detected"
        )]
        fn relate(&self, _older: &str, _newer: &str) -> Result<RelationVerdict, SemanticsError> {
            panic!("relater must not be called for gated-out pairs");
        }
        fn fingerprint(&self) -> String {
            "never".to_owned()
        }
    }

    #[test]
    fn relate_prefilter_skips_low_similarity_pairs_within_a_cluster() {
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "alpha subject", 1),
            claim("claim_bbbbbbbbbbbb", "x", "beta subject", 2),
        ];
        // Cosine ~0.30: above the cluster link threshold (0.2), so both claims
        // share one cluster — but below the prefilter (0.5), so the pair must
        // still be gated out before any judge call.
        let embedder = FixedEmbedder::new(
            vec![("alpha", vec![1.0, 0.0]), ("beta", vec![0.3, 0.954])],
            2,
        );
        let out = relate_claims(&claims, &embedder, &NeverRelater, th(0.2, 0.5)).expect("relate");
        assert!(out.supersessions.is_empty());
        assert!(out.conflicts.is_empty());
    }

    #[test]
    fn relate_skips_cross_cluster_pairs_without_judging() {
        // Cosine ~0.87: the pair clears the coarse prefilter (0.6) — under the
        // pre-clustering pipeline it WOULD have been judged — but it falls below
        // the cluster link threshold (0.95), so the two claims land in different
        // clusters and the pair is deliberately never judged. This is the
        // documented cross-cluster skip that bounds judge calls.
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "alpha subject", 1),
            claim("claim_bbbbbbbbbbbb", "x", "beta subject", 2),
        ];
        let embedder = FixedEmbedder::new(
            vec![("alpha", vec![1.0, 0.0]), ("beta", vec![0.87, 0.493])],
            2,
        );
        let out = relate_claims(&claims, &embedder, &NeverRelater, th(0.95, 0.6)).expect("relate");
        assert!(out.supersessions.is_empty());
        assert!(out.conflicts.is_empty());
    }

    #[test]
    fn relate_empty_and_singleton_inputs_are_inert() {
        let embedder = FixedEmbedder::new(Vec::new(), 2);
        let relater = ScriptedRelater::new(Vec::new());
        let empty = relate_claims(&[], &embedder, &relater, th(0.5, 0.5)).expect("relate empty");
        assert!(empty.supersessions.is_empty() && empty.conflicts.is_empty());

        let one = vec![claim("claim_aaaaaaaaaaaa", "x", "Deploys on Tuesday", 1)];
        let single = relate_claims(&one, &embedder, &relater, th(0.5, 0.5)).expect("relate one");
        assert!(single.supersessions.is_empty() && single.conflicts.is_empty());
    }

    #[test]
    fn relate_propagates_embedder_failure() {
        struct FailingEmbedder;
        impl Embedder for FailingEmbedder {
            fn embed(&self, _text: &str) -> Result<Vec<f32>, SemanticsError> {
                Err(SemanticsError::DimensionMismatch {
                    expected: 2,
                    actual: 1,
                })
            }
        }
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "one", 1),
            claim("claim_bbbbbbbbbbbb", "x", "two", 2),
        ];
        let relater = ScriptedRelater::new(Vec::new());
        let err = relate_claims(&claims, &FailingEmbedder, &relater, th(0.5, 0.5))
            .expect_err("propagate");
        assert!(matches!(err, PipelineError::Semantics(_)));
    }

    // --- cluster-first candidate generation (the O(n²) fix) ---

    /// Deterministic relater that counts every judge call and always answers
    /// [`ClaimRelation::Unrelated`], so candidate generation alone decides how
    /// many calls happen.
    struct CountingRelater {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl CountingRelater {
        fn new() -> Self {
            Self {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
        fn count(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl ClaimRelater for CountingRelater {
        fn relate(&self, _older: &str, _newer: &str) -> Result<RelationVerdict, SemanticsError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(RelationVerdict {
                relation: ClaimRelation::Unrelated,
                score: 1.0,
            })
        }
        fn fingerprint(&self) -> String {
            "counting".to_owned()
        }
    }

    /// Corpus of `k = 3` well-separated clusters of size `m = 3` (n = 9), as 2-D
    /// unit vectors at angular offsets: within-cluster spread ≤ 2° (cosine
    /// ≥ 0.999), adjacent clusters 30° apart (cosine ≈ 0.85–0.88 — above the 0.6
    /// prefilter, below the 0.98 cluster threshold), far clusters 60° apart
    /// (cosine < 0.6).
    fn three_cluster_corpus() -> (Vec<(ClaimId, ClaimView)>, FixedEmbedder) {
        // (distinct keyword, degrees) — no keyword is a substring of another.
        let spec: [(&str, f32); 9] = [
            ("aardvark", 0.0),
            ("abacus", 1.0),
            ("acorn", 2.0),
            ("baboon", 30.0),
            ("badger", 31.0),
            ("bagel", 32.0),
            ("cactus", 60.0),
            ("camel", 61.0),
            ("candle", 62.0),
        ];
        let table: Vec<(&'static str, Vec<f32>)> = spec
            .iter()
            .map(|(key, deg)| {
                let rad = deg.to_radians();
                (*key, vec![rad.cos(), rad.sin()])
            })
            .collect();
        let claims: Vec<(ClaimId, ClaimView)> = spec
            .iter()
            .enumerate()
            .map(|(idx, (key, _))| {
                let seq = u64::try_from(idx).expect("small index") + 1;
                let id = format!("claim_{idx:012x}");
                claim(&id, "x", &format!("the {key} subject"), seq)
            })
            .collect();
        (claims, FixedEmbedder::new(table, 2))
    }

    const CORPUS_THRESHOLDS: RelateThresholds = RelateThresholds {
        cluster: 0.98,
        prefilter: 0.6,
    };

    #[test]
    fn judge_calls_bounded_by_within_cluster_pairs() {
        let (claims, embedder) = three_cluster_corpus();
        let n = claims.len();

        // Sanity: the corpus clusters as 3 components of 3 at the cluster
        // threshold, so the within-cluster pair bound is Σ (3 choose 2) = 9.
        let groups = group_claims(&claims, &embedder, CORPUS_THRESHOLDS.cluster).expect("group");
        let sizes: Vec<usize> = groups.iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![3, 3, 3]);
        let within_cluster_pairs: usize = sizes.iter().map(|m| m * (m - 1) / 2).sum();

        // The pre-clustering pipeline judged every pair clearing the prefilter —
        // count those pairs to show the bound is a strict improvement here.
        let texts: Vec<&str> = claims.iter().map(|(_, v)| v.text.as_str()).collect();
        let embeddings = embedder.embed_batch(&texts).expect("embed");
        let mut prefilter_pairs = 0usize;
        for i in 0..n {
            for j in (i + 1)..n {
                if cosine_similarity(&embeddings[i], &embeddings[j]) >= CORPUS_THRESHOLDS.prefilter
                {
                    prefilter_pairs += 1;
                }
            }
        }

        let relater = CountingRelater::new();
        let out = relate_claims(&claims, &embedder, &relater, CORPUS_THRESHOLDS).expect("relate");
        assert!(out.supersessions.is_empty() && out.conflicts.is_empty());

        assert!(
            relater.count() <= within_cluster_pairs,
            "judge calls ({}) must be bounded by Σ (m_i choose 2) = {within_cluster_pairs}",
            relater.count()
        );
        assert_eq!(
            relater.count(),
            within_cluster_pairs,
            "every distinct-text within-cluster pair passes the prefilter here"
        );
        assert!(
            relater.count() < prefilter_pairs,
            "clustering must judge strictly fewer pairs than the prefilter alone \
             ({} vs {prefilter_pairs})",
            relater.count()
        );
        assert!(
            prefilter_pairs < n * (n - 1) / 2,
            "sanity: some pairs fall below the prefilter too"
        );
    }

    #[test]
    fn clustering_is_deterministic_across_runs() {
        let (claims, embedder) = three_cluster_corpus();
        let first = group_claims(&claims, &embedder, CORPUS_THRESHOLDS.cluster).expect("group");
        for _ in 0..5 {
            let again = group_claims(&claims, &embedder, CORPUS_THRESHOLDS.cluster).expect("group");
            assert_eq!(first, again, "same input must yield identical clusters");
        }
        // Stable ordering: members ascend within a group; groups are ordered by
        // their first (smallest) member index — never hash-map iteration order.
        for group in &first {
            assert!(group.windows(2).all(|w| w[0] < w[1]));
        }
        let firsts: Vec<usize> = first.iter().map(|g| g[0]).collect();
        assert!(firsts.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn relate_is_deterministic_across_runs() {
        let (claims, embedder) = three_cluster_corpus();
        // Script a supersession and a conflict inside two different clusters so
        // both output vectors are non-empty.
        let relater = ScriptedRelater::new(vec![
            ("aardvark", "acorn", ClaimRelation::Supersedes),
            ("baboon", "bagel", ClaimRelation::Conflict),
        ]);
        let first = relate_claims(&claims, &embedder, &relater, CORPUS_THRESHOLDS).expect("relate");
        assert_eq!(first.supersessions.len(), 1);
        assert_eq!(first.conflicts.len(), 1);
        for _ in 0..5 {
            let again =
                relate_claims(&claims, &embedder, &relater, CORPUS_THRESHOLDS).expect("relate");
            assert_eq!(
                first.supersessions, again.supersessions,
                "supersessions must be identical across runs"
            );
            let ids = |out: &RelatedClaims| -> Vec<String> {
                out.conflicts
                    .iter()
                    .map(|c| c.conflict_id.to_string())
                    .collect()
            };
            assert_eq!(
                ids(&first),
                ids(&again),
                "conflict ids must be identical across runs"
            );
        }
    }

    #[test]
    fn unresolved_pair_taints_and_holds_dependent_supersession() {
        struct PartiallyFailingRelater;
        impl ClaimRelater for PartiallyFailingRelater {
            fn relate(&self, older: &str, newer: &str) -> Result<RelationVerdict, SemanticsError> {
                if older.contains("alpha") && newer.contains("beta") {
                    return Err(SemanticsError::Backend {
                        source: Box::new(std::io::Error::other("judge failed")),
                    });
                }
                Ok(RelationVerdict {
                    relation: if older.contains("alpha") && newer.contains("gamma") {
                        ClaimRelation::Supersedes
                    } else {
                        ClaimRelation::Unrelated
                    },
                    score: 1.0,
                })
            }

            fn fingerprint(&self) -> String {
                "partial".to_string()
            }
        }

        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "alpha old", 1),
            claim("claim_bbbbbbbbbbbb", "x", "beta uncertain", 2),
            claim("claim_cccccccccccc", "x", "gamma winner", 3),
        ];
        let embedder = FixedEmbedder::new(
            vec![
                ("alpha", vec![1.0, 0.0]),
                ("beta", vec![1.0, 0.0]),
                ("gamma", vec![1.0, 0.0]),
            ],
            2,
        );
        let outcome = relate_claims(&claims, &embedder, &PartiallyFailingRelater, th(0.9, 0.9))
            .expect("partial outcome");
        assert_eq!(outcome.unresolved.len(), 1);
        assert!(outcome.related.supersessions.is_empty());
        assert!(matches!(
            outcome.held.as_slice(),
            [HeldDecision::Supersession { old_claim, new_claim, .. }]
                if old_claim.as_str() == "claim_aaaaaaaaaaaa"
                    && new_claim.as_str() == "claim_cccccccccccc"
        ));
    }

    #[test]
    fn authoritative_pair_is_reused_without_judge_call() {
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "alpha", 1),
            claim("claim_bbbbbbbbbbbb", "x", "beta", 2),
            claim("claim_cccccccccccc", "x", "gamma", 3),
        ];
        let embedder = FixedEmbedder::new(
            vec![
                ("alpha", vec![1.0, 0.0]),
                ("beta", vec![1.0, 0.0]),
                ("gamma", vec![1.0, 0.0]),
            ],
            2,
        );
        let relater = CountingRelater::new();
        let mut settled = BTreeMap::new();
        settled.insert(
            (claims[0].0.clone(), claims[1].0.clone()),
            RelationVerdict {
                relation: ClaimRelation::Unrelated,
                score: 1.0,
            },
        );
        let outcome = relate_claims_with_settled(
            &claims,
            &embedder,
            &relater,
            th(0.9, 0.9),
            &settled,
            Duration::MAX,
        )
        .expect("outcome");
        assert_eq!(relater.count(), 2);
        assert_eq!(
            outcome
                .related
                .judgments
                .iter()
                .filter(|judgment| judgment.reused_authority)
                .count(),
            1
        );
    }

    /// Verdict depends only on the text pair, so a correct parallel run must
    /// produce byte-identical output to the sequential run; call count exposes
    /// duplicate model calls for identical-text pairs.
    struct TextKeyedCountingRelater {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl TextKeyedCountingRelater {
        fn new() -> Self {
            Self {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
        fn count(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl ClaimRelater for TextKeyedCountingRelater {
        fn relate(&self, _older: &str, newer: &str) -> Result<RelationVerdict, SemanticsError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // Deterministic in the text pair: "supersede" wording => Supersedes.
            let relation = if newer.contains("moved") {
                ClaimRelation::Supersedes
            } else {
                ClaimRelation::Unrelated
            };
            Ok(RelationVerdict {
                relation,
                score: 1.0,
            })
        }
        fn fingerprint(&self) -> String {
            "text-keyed-counting".to_owned()
        }
    }

    /// Two clusters, each three same-day claims: many intra-cluster pairs feed
    /// the fan-out so worker interleaving is genuinely exercised.
    fn parallel_fanout_corpus() -> (Vec<(ClaimId, ClaimView)>, FixedEmbedder) {
        let claims = vec![
            claim(
                "claim_0000000000a1",
                "deploy",
                "Deploys happen on Friday",
                1,
            ),
            claim(
                "claim_0000000000a2",
                "deploy",
                "Deploys moved to Wednesday",
                2,
            ),
            claim(
                "claim_0000000000a3",
                "deploy",
                "Deploys moved to Tuesday",
                3,
            ),
            claim(
                "claim_0000000000b1",
                "release",
                "Releases happen on Monday",
                4,
            ),
            claim(
                "claim_0000000000b2",
                "release",
                "Releases moved to Thursday",
                5,
            ),
            claim(
                "claim_0000000000b3",
                "release",
                "Releases moved to Saturday",
                6,
            ),
        ];
        let embedder = FixedEmbedder::new(
            vec![
                ("deploys happen on friday", vec![1.0, 0.0]),
                ("deploys moved to wednesday", vec![0.999, 0.01]),
                ("deploys moved to tuesday", vec![0.998, 0.02]),
                ("releases happen on monday", vec![0.0, 1.0]),
                ("releases moved to thursday", vec![0.01, 0.999]),
                ("releases moved to saturday", vec![0.02, 0.998]),
            ],
            2,
        );
        (claims, embedder)
    }

    #[test]
    fn parallel_relate_equals_sequential_output() {
        let (claims, embedder) = parallel_fanout_corpus();
        let seq = relate_claims_with_settled(
            &claims,
            &embedder,
            &TextKeyedCountingRelater::new(),
            th(0.9, 0.6),
            &BTreeMap::new(),
            Duration::MAX,
        )
        .expect("sequential");
        for concurrency in [2_usize, 4, 8] {
            let par = relate_claims_settled_parallel(
                &claims,
                &embedder,
                &TextKeyedCountingRelater::new(),
                th(0.9, 0.6),
                &BTreeMap::new(),
                Duration::MAX,
                concurrency,
            )
            .expect("parallel");
            assert_eq!(
                format!("{seq:?}"),
                format!("{par:?}"),
                "parallel output diverged at concurrency {concurrency}"
            );
        }
    }

    #[test]
    fn parallel_relate_coalesces_duplicate_text_pairs_to_one_call() {
        // Four logical claim ids but only two distinct text pairs among the
        // candidate pairs: identical texts must judge exactly once.
        let claims = vec![
            claim("claim_00000000dup1", "x", "The service uses Postgres", 1),
            claim("claim_00000000dup2", "x", "The service uses Postgres", 2),
            claim("claim_00000000dup3", "x", "The service uses Redis", 3),
            claim("claim_00000000dup4", "x", "The service uses Redis", 4),
        ];
        let embedder = FixedEmbedder::new(
            vec![
                ("the service uses postgres", vec![1.0, 0.0]),
                ("the service uses redis", vec![0.99, 0.02]),
            ],
            2,
        );
        let relater = TextKeyedCountingRelater::new();
        let out = relate_claims_settled_parallel(
            &claims,
            &embedder,
            &relater,
            th(0.9, 0.6),
            &BTreeMap::new(),
            Duration::MAX,
            8,
        )
        .expect("parallel");
        // All four claims cluster (postgres ~= redis at 0.9998). The two
        // same-text pairs (postgres,postgres) and (redis,redis) are dropped by
        // the normalized-text guard, leaving four distinct LOGICAL cross-pairs
        // that all carry the identical (postgres, redis) text pair. Coalescing
        // must judge that text pair exactly ONCE and fan the verdict to all
        // four — without it a cold parallel run would make four paid calls.
        assert_eq!(
            relater.count(),
            1,
            "identical-text logical pairs must coalesce to a single model call"
        );
        assert_eq!(
            out.related.judgments.len(),
            4,
            "every surviving logical pair receives the shared verdict"
        );
        assert!(out.unresolved.is_empty());

        // Sanity: the sequential path with the same stub (no disk cache) makes
        // one call per pair — proving the parallel coalescing, not the corpus,
        // is what collapses the calls.
        let seq_relater = TextKeyedCountingRelater::new();
        let _ = relate_claims_with_settled(
            &claims,
            &embedder,
            &seq_relater,
            th(0.9, 0.6),
            &BTreeMap::new(),
            Duration::MAX,
        )
        .expect("sequential");
        assert_eq!(
            seq_relater.count(),
            4,
            "uncached sequential path judges each of the four logical pairs"
        );
    }

    #[test]
    fn parallel_relate_preserves_settlement_and_holdback() {
        let (claims, embedder) = parallel_fanout_corpus();
        // Pre-settle one pair from journal authority; it must not be re-judged.
        let mut settled = BTreeMap::new();
        settled.insert(
            (claims[0].0.clone(), claims[1].0.clone()),
            RelationVerdict {
                relation: ClaimRelation::Supersedes,
                score: 1.0,
            },
        );
        let relater = TextKeyedCountingRelater::new();
        let out = relate_claims_settled_parallel(
            &claims,
            &embedder,
            &relater,
            th(0.9, 0.6),
            &settled,
            Duration::MAX,
            4,
        )
        .expect("parallel");
        assert_eq!(
            out.related
                .judgments
                .iter()
                .filter(|judgment| judgment.reused_authority)
                .count(),
            1,
            "the settled pair is reused, not re-judged"
        );
    }

    /// A relater that forces calls to complete in reverse dispatch order. If
    /// reduction depended on completion order the output would flip; proving
    /// byte-identity under this adversarial schedule witnesses the fan-out's
    /// deterministic reassembly contract.
    struct ReverseOrderRelater {
        gate: std::sync::Barrier,
        ranks: BTreeMap<(String, String), usize>,
        state: std::sync::Mutex<ReverseOrderState>,
        ready: std::sync::Condvar,
    }

    struct ReverseOrderState {
        next_rank: Option<usize>,
        completed: Vec<usize>,
    }

    impl ReverseOrderRelater {
        fn new(ordered_pairs: Vec<(String, String)>) -> Self {
            let total = ordered_pairs.len();
            let ranks = ordered_pairs
                .into_iter()
                .enumerate()
                .map(|(rank, pair)| (pair, rank))
                .collect();
            Self {
                gate: std::sync::Barrier::new(total),
                ranks,
                state: std::sync::Mutex::new(ReverseOrderState {
                    next_rank: total.checked_sub(1),
                    completed: Vec::with_capacity(total),
                }),
                ready: std::sync::Condvar::new(),
            }
        }

        fn completed(&self) -> Vec<usize> {
            self.state
                .lock()
                .expect("reverse-order state lock")
                .completed
                .clone()
        }
    }

    impl ClaimRelater for ReverseOrderRelater {
        fn relate(&self, older: &str, newer: &str) -> Result<RelationVerdict, SemanticsError> {
            let rank = *self
                .ranks
                .get(&(older.to_string(), newer.to_string()))
                .expect("pair has a dispatch rank");
            // All representatives must be in flight before the highest rank is
            // released. Each completion then unlocks exactly the preceding
            // rank, forcing N-1..0 without sleeps or scheduler assumptions.
            self.gate.wait();
            let mut state = self.state.lock().expect("reverse-order state lock");
            while state.next_rank != Some(rank) {
                state = self.ready.wait(state).expect("reverse-order wait");
            }
            state.completed.push(rank);
            state.next_rank = rank.checked_sub(1);
            self.ready.notify_all();
            drop(state);
            let relation = if newer.contains("moved") {
                ClaimRelation::Supersedes
            } else {
                ClaimRelation::Unrelated
            };
            Ok(RelationVerdict {
                relation,
                score: 1.0,
            })
        }
        fn fingerprint(&self) -> String {
            "reverse-order".to_owned()
        }
    }

    #[test]
    fn parallel_reassembly_is_independent_of_completion_order() {
        let (claims, embedder) = parallel_fanout_corpus();
        let seq = relate_claims_with_settled(
            &claims,
            &embedder,
            &TextKeyedCountingRelater::new(),
            th(0.9, 0.6),
            &BTreeMap::new(),
            Duration::MAX,
        )
        .expect("sequential");

        let pending = prepare_pairs(
            &claims,
            &embedder,
            th(0.9, 0.6),
            &BTreeMap::new(),
            &RelateTemporalPolicy::default(),
        )
        .expect("prepare")
        .expect("non-empty candidates");
        let mut seen = BTreeSet::new();
        let ordered_pairs = pending
            .iter()
            .filter_map(|pair| {
                let texts = (
                    claims[pair.old_idx].1.text.clone(),
                    claims[pair.new_idx].1.text.clone(),
                );
                seen.insert(texts.clone()).then_some(texts)
            })
            .collect::<Vec<_>>();
        let representative_calls = ordered_pairs.len();
        assert!(
            representative_calls >= 2,
            "need real fan-out to test ordering"
        );

        let relater = ReverseOrderRelater::new(ordered_pairs);
        let par = relate_claims_settled_parallel(
            &claims,
            &embedder,
            &relater,
            th(0.9, 0.6),
            &BTreeMap::new(),
            Duration::MAX,
            representative_calls,
        )
        .expect("parallel");
        assert_eq!(
            format!("{seq:?}"),
            format!("{par:?}"),
            "reassembly must not depend on worker completion order"
        );
        assert_eq!(
            relater.completed(),
            (0..representative_calls).rev().collect::<Vec<_>>(),
            "test harness must actually force reverse completion order"
        );
    }

    #[test]
    fn git_ancestry_orients_pairs_instead_of_ingest_sequence() {
        let claims = vec![
            claim(
                "claim_aaaaaaaaaaaa",
                "storage",
                "old storage uses postgres",
                20,
            ),
            claim(
                "claim_bbbbbbbbbbbb",
                "storage",
                "new storage uses batpak",
                10,
            ),
        ];
        let embedder = FixedEmbedder::new(vec![("storage", vec![1.0, 0.0])], 2);
        let relater = ScriptedRelater::new(vec![(
            "old storage",
            "new storage",
            ClaimRelation::Supersedes,
        )]);
        let left = SourceSnapshotId::derive("ancestor");
        let right = SourceSnapshotId::derive("descendant");
        let mut temporal = RelateTemporalPolicy::default();
        temporal.bind_claim(&claims[0].0, &left);
        temporal.bind_claim(&claims[1].0, &right);
        temporal.insert_relation(&left, &right, TemporalRelation::Before);

        let out = relate_claims_with_settled_temporal(
            &claims,
            &embedder,
            &relater,
            th(0.9, 0.6),
            &BTreeMap::new(),
            &temporal,
            Duration::MAX,
        )
        .expect("relate");

        assert_eq!(
            out.supersessions,
            vec![(
                claims[0].0.clone(),
                claims[1].0.clone(),
                "superseded by x.md:10".to_string(),
            )]
        );
        assert_eq!(out.related.judgments[0].older_claim, claims[0].0);
        assert_eq!(out.related.judgments[0].newer_claim, claims[1].0);
    }

    #[test]
    fn incomparable_or_missing_source_order_never_calls_the_judge() {
        for (relation, expected) in [
            (
                Some(TemporalRelation::Concurrent),
                RelationFailureClass::TemporalConcurrent,
            ),
            (None, RelationFailureClass::TemporalUnknown),
        ] {
            let claims = vec![
                claim(
                    "claim_aaaaaaaaaaaa",
                    "storage",
                    "old storage uses postgres",
                    1,
                ),
                claim(
                    "claim_bbbbbbbbbbbb",
                    "storage",
                    "new storage uses batpak",
                    2,
                ),
            ];
            let embedder = FixedEmbedder::new(vec![("storage", vec![1.0, 0.0])], 2);
            let relater = CountingRelater::new();
            let left = SourceSnapshotId::derive("left");
            let right = SourceSnapshotId::derive("right");
            let mut temporal = RelateTemporalPolicy::default();
            temporal.bind_claim(&claims[0].0, &left);
            temporal.bind_claim(&claims[1].0, &right);
            if let Some(relation) = relation {
                temporal.insert_relation(&left, &right, relation);
            }

            let out = relate_claims_settled_parallel_temporal(
                &claims,
                &embedder,
                &relater,
                th(0.9, 0.6),
                &BTreeMap::new(),
                ParallelRelateOptions {
                    temporal: &temporal,
                    budget: Duration::MAX,
                    concurrency: 4,
                },
            )
            .expect("relate");

            assert_eq!(relater.count(), 0);
            assert!(out.related.judgments.is_empty());
            assert_eq!(out.unresolved.len(), 1);
            assert_eq!(out.unresolved[0].failure.class, expected);
            assert!(out.supersessions.is_empty() && out.conflicts.is_empty());
        }
    }

    #[test]
    fn journal_authority_keeps_its_original_pair_direction() {
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "storage", "storage alpha", 1),
            claim("claim_bbbbbbbbbbbb", "storage", "storage beta", 2),
        ];
        let embedder = FixedEmbedder::new(vec![("storage", vec![1.0, 0.0])], 2);
        let relater = CountingRelater::new();
        let left = SourceSnapshotId::derive("left");
        let right = SourceSnapshotId::derive("right");
        let mut temporal = RelateTemporalPolicy::default();
        temporal.bind_claim(&claims[0].0, &left);
        temporal.bind_claim(&claims[1].0, &right);
        temporal.insert_relation(&left, &right, TemporalRelation::After);
        let mut settled = BTreeMap::new();
        settled.insert(
            (claims[0].0.clone(), claims[1].0.clone()),
            RelationVerdict {
                relation: ClaimRelation::Unrelated,
                score: 1.0,
            },
        );

        let out = relate_claims_settled_parallel_temporal(
            &claims,
            &embedder,
            &relater,
            th(0.9, 0.6),
            &settled,
            ParallelRelateOptions {
                temporal: &temporal,
                budget: Duration::MAX,
                concurrency: 4,
            },
        )
        .expect("relate");

        assert_eq!(relater.count(), 0);
        assert_eq!(out.related.judgments[0].older_claim, claims[0].0);
        assert_eq!(out.related.judgments[0].newer_claim, claims[1].0);
        assert!(out.related.judgments[0].reused_authority);
    }
}
