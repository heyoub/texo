//! Semantic supersession and conflict logic.
//!
//! This module replaces the exact `subject_hint` bucketing + replacement-keyword
//! supersession + brittle contradiction-signal pile (see [`crate::stale::check`]
//! and [`crate::conflicts::detect`]) with **meaning-based** logic driven by two
//! injected backends:
//!
//! * an [`Embedder`] — used for deterministic, cursor-paged candidate
//!   generation. Each pass examines a hard-bounded slice of the global pair
//!   space and only pairs clearing the configured semantic floor reach the
//!   judge;
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

use std::collections::BTreeMap;

#[cfg(test)]
use std::collections::BTreeSet;
#[cfg(test)]
use std::time::Duration;

use crate::events::ids::{ClaimId, ConflictId, SourceId};
use crate::knowledge::{SourceSnapshotId, TemporalRelation};
#[cfg(test)]
use crate::relate::settlement::RelationFailureClass;
use crate::relate::settlement::{HeldDecision, UnresolvedPair};
#[cfg(test)]
use crate::semantics::ClaimRelation;
use crate::semantics::{cosine_similarity, Embedder, SemanticsError};

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
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the wrapped sequence.
    #[must_use]
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
    /// Corpus similarity floor. Candidate paging uses the lower of this value
    /// and [`Self::prefilter`] to preserve recall while bounding work.
    pub cluster: f32,
    /// Coarse per-pair cosine prefilter. The relater performs the final semantic
    /// classification, so this value is a recall gate rather than an authority.
    pub prefilter: f32,
}

/// Default maximum number of global pair slots examined by one relate
/// pass. The bound applies to settled and unsettled pairs alike.
pub const DEFAULT_CANDIDATE_PAIR_BUDGET: usize = 4_096;

/// Opaque deterministic cursor into global pair enumeration.
///
/// A cursor counts raw pair slots, before the cosine prefilter and duplicate
/// text gate. Consequently a page examines at most its configured budget even
/// when most pairs are filtered or already settled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct CandidateCursor(u64);

impl CandidateCursor {
    /// Start at the first global pair.
    #[must_use]
    pub const fn start() -> Self {
        Self(0)
    }

    /// Restore a cursor returned by a prior partial outcome.
    #[must_use]
    pub const fn from_offset(offset: u64) -> Self {
        Self(offset)
    }

    /// Return the stable wire representation.
    #[must_use]
    pub const fn offset(self) -> u64 {
        self.0
    }
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

    pub(crate) fn compare_claims(
        &self,
        left: &ClaimId,
        right: &ClaimId,
    ) -> Option<TemporalRelation> {
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

    /// Stable identity of every durable input that can change temporal pair orientation.
    #[must_use]
    pub(crate) fn campaign_identity_digest(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        hash_identity_field(&mut hasher, b"texo.relate.temporal-policy.v1");
        for (claim_id, snapshot_id) in &self.claim_snapshots {
            hash_identity_field(&mut hasher, claim_id.as_bytes());
            hash_identity_field(&mut hasher, snapshot_id.as_bytes());
        }
        for ((left, right), relation) in &self.snapshot_relations {
            hash_identity_field(&mut hasher, left.as_bytes());
            hash_identity_field(&mut hasher, right.as_bytes());
            hasher.update(&[temporal_relation_tag(*relation)]);
        }
        hasher.finalize().to_hex().to_string()
    }
}

fn hash_identity_field(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_be_bytes());
    hasher.update(value);
}

const fn temporal_relation_tag(relation: TemporalRelation) -> u8 {
    match relation {
        TemporalRelation::Before => 1,
        TemporalRelation::After => 2,
        TemporalRelation::Same => 3,
        TemporalRelation::Concurrent => 4,
        TemporalRelation::Unknown => 5,
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

/// A completed semantic pass. Authority-bearing derived edges exist only here.
#[derive(Debug, Default)]
pub struct CompleteRelateOutcome {
    /// Decisions reduced from the complete authoritative verdict set.
    pub related: RelatedClaims,
    /// Decisions withheld by semantic ambiguity rather than pagination.
    pub held: Vec<HeldDecision>,
    /// Total authoritative candidate judgments reduced for this result.
    pub candidate_pairs: usize,
    /// Pair-slot examination ceiling used by the completing page.
    pub candidate_pair_budget: usize,
}

/// An incomplete semantic pass. It deliberately cannot carry publishable
/// supersession or conflict edges.
#[derive(Debug)]
pub struct PartialRelateOutcome {
    /// Successful judgments acquired on this bounded page. Callers journal
    /// these before resuming.
    pub judgments: Vec<PairJudgment>,
    /// Candidate pairs on this page for which no verdict exists.
    pub unresolved: Vec<UnresolvedPair>,
    /// Every derived edge is represented as held until completion.
    pub held: Vec<HeldDecision>,
    /// Number of raw global pair slots examined on this page.
    pub candidate_pairs: usize,
    /// Pair-slot examination ceiling used by this page.
    pub candidate_pair_budget: usize,
    /// Cursor for the next bounded page or earliest unresolved pair.
    pub next_candidate_cursor: CandidateCursor,
}

/// Typed complete-or-partial semantic pipeline result.
///
/// The enum shape makes publishing derived authority from a partial pass
/// unrepresentable: only [`CompleteRelateOutcome`] contains those edges.
#[derive(Debug)]
pub enum RelateOutcome {
    /// All candidate pages and verdicts are complete.
    Complete(CompleteRelateOutcome),
    /// More bounded work or a retry is required.
    Partial(PartialRelateOutcome),
}

impl Default for RelateOutcome {
    fn default() -> Self {
        Self::Complete(CompleteRelateOutcome {
            candidate_pair_budget: DEFAULT_CANDIDATE_PAIR_BUDGET,
            ..CompleteRelateOutcome::default()
        })
    }
}

impl RelateOutcome {
    /// Borrow the complete state when all candidate pages settled.
    #[must_use]
    pub const fn complete(&self) -> Option<&CompleteRelateOutcome> {
        match self {
            Self::Complete(outcome) => Some(outcome),
            Self::Partial(_) => None,
        }
    }

    /// Borrow the partial state when more work is required.
    #[must_use]
    pub const fn partial(&self) -> Option<&PartialRelateOutcome> {
        match self {
            Self::Partial(outcome) => Some(outcome),
            Self::Complete(_) => None,
        }
    }

    /// Successful judgments acquired by this invocation.
    #[must_use]
    pub fn judgments(&self) -> &[PairJudgment] {
        match self {
            Self::Complete(outcome) => &outcome.related.judgments,
            Self::Partial(outcome) => &outcome.judgments,
        }
    }

    /// Unresolved pairs, present only for a partial result.
    #[must_use]
    pub fn unresolved(&self) -> &[UnresolvedPair] {
        match self {
            Self::Complete(_) => &[],
            Self::Partial(outcome) => &outcome.unresolved,
        }
    }

    /// Held decisions for either completion state.
    #[must_use]
    pub fn held(&self) -> &[HeldDecision] {
        match self {
            Self::Complete(outcome) => &outcome.held,
            Self::Partial(outcome) => &outcome.held,
        }
    }

    /// True when another bounded page or retry is required.
    #[must_use]
    pub const fn is_partial(&self) -> bool {
        matches!(self, Self::Partial(_))
    }
}

mod candidate;
mod reduction;
mod runtime;

#[cfg(test)]
use candidate::prepare_pairs;
pub(crate) use runtime::classify_pair_failure;
pub use runtime::{
    relate_claims, relate_claims_settled_parallel, relate_claims_settled_parallel_temporal,
    relate_claims_with_settled, relate_claims_with_settled_temporal, ParallelRelateOptions,
};
#[cfg(test)]
mod tests;
