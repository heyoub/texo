//! Semantic grouping, supersession, and conflict logic.
//!
//! This module replaces the exact `subject_hint` bucketing + replacement-keyword
//! supersession + brittle contradiction-signal pile (see [`crate::stale::check`]
//! and [`crate::conflicts::detect`]) with **meaning-based** logic driven by two
//! injected backends:
//!
//! * an [`Embedder`] — claims whose embeddings have cosine similarity at or above
//!   a caller-supplied threshold are grouped into the same subject cluster, so
//!   "release approval" (who) and "release schedule" (which day) split apart even
//!   though both contain the word "release";
//! * an [`Nli`] — within a group, a *newer* claim **supersedes** an older one when
//!   it entails it, and two co-current claims **conflict** when they mutually
//!   contradict.
//!
//! Every function here is **pure**: it takes the claims and the backends and
//! returns plain data, performing no I/O. The backends are trait objects so the
//! logic can be proven deterministically with in-test stubs (no model, no
//! network). These functions are **additive and opt-in** — they mirror the
//! output shapes of [`crate::stale::check::infer_supersessions`] and
//! [`crate::conflicts::detect::detect_conflicts`] so a later step can swap them in
//! behind configuration without changing default behavior.

use std::collections::HashSet;

use crate::replay::state::ClaimView;
use crate::semantics::{cosine_similarity, Embedder, Entailment, Nli, SemanticsError};
use crate::state::conflict_lifecycle::ConflictEntry;
use crate::types::ids::{conflict_id_from_pair, ClaimId};
use crate::types::status::ConflictStatus;

/// Failure raised while running the semantic pipeline.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// A backend ([`Embedder`] or [`Nli`]) failed.
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
pub fn group_claims(
    claims: &[(ClaimId, ClaimView)],
    embedder: &dyn Embedder,
    threshold: f32,
) -> Result<Vec<Vec<usize>>, PipelineError> {
    let n = claims.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    let texts: Vec<&str> = claims.iter().map(|(_, v)| embedding_text(v)).collect();
    let embeddings = embedder.embed_batch(&texts)?;

    // Union-find over claim indices.
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
    Ok(groups)
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

/// Infer supersession edges by meaning.
///
/// Claims are clustered with [`group_claims`]. Within each group, for every
/// ordered pair `(older, newer)` (by journal/sequence order, recency as the
/// tiebreaker), the newer claim **supersedes** the older one when
/// `Nli.classify(premise = newer, hypothesis = older)` is [`Entailment`] — the
/// newer claim covers/entails the older. Identical-text pairs are skipped
/// (duplicates, not supersessions); only a newer claim may win.
///
/// Each older claim is superseded at most once, by the newest entailing claim, so
/// a single canonical winner emerges per stale claim (e.g. Friday -> Tuesday, not
/// both Friday -> Wednesday and Friday -> Tuesday).
///
/// Mirrors [`crate::stale::check::infer_supersessions`]'s `(old, new, reason)`
/// edge shape so it can be swapped in behind configuration later.
pub fn infer_supersessions_semantic(
    claims: &[(ClaimId, ClaimView)],
    embedder: &dyn Embedder,
    nli: &dyn Nli,
    threshold: f32,
) -> Result<Vec<SupersessionEdge>, PipelineError> {
    let groups = group_claims(claims, embedder, threshold)?;
    let mut edges: Vec<SupersessionEdge> = Vec::new();

    for group in &groups {
        if group.len() < 2 {
            continue;
        }
        // Order members oldest -> newest; ties broken by index for determinism.
        let mut ordered: Vec<usize> = group.clone();
        ordered.sort_by(|&a, &b| {
            sequence_rank(&claims[a].1)
                .cmp(&sequence_rank(&claims[b].1))
                .then(a.cmp(&b))
        });

        for (pos, &old_idx) in ordered.iter().enumerate() {
            let (old_id, old_view) = &claims[old_idx];
            // Find the newest later claim that entails this one.
            let mut winner: Option<usize> = None;
            for &new_idx in ordered.iter().skip(pos + 1) {
                let (_, new_view) = &claims[new_idx];
                if new_view.normalized_text == old_view.normalized_text {
                    continue;
                }
                let verdict = nli.classify(embedding_text(new_view), embedding_text(old_view))?;
                if verdict.label == Entailment::Entailment {
                    winner = Some(new_idx);
                }
            }
            if let Some(new_idx) = winner {
                let (new_id, new_view) = &claims[new_idx];
                edges.push((
                    old_id.clone(),
                    new_id.clone(),
                    format!(
                        "superseded by {}:{}",
                        new_view.source_path, new_view.line_start
                    ),
                ));
            }
        }
    }

    edges.sort_by(|a, b| {
        a.0.as_str()
            .cmp(b.0.as_str())
            .then_with(|| a.1.as_str().cmp(b.1.as_str()))
    });
    Ok(edges)
}

/// Detect conflicts by meaning.
///
/// Claims are clustered with [`group_claims`]. Within each group, an unordered
/// pair `(a, b)` is a **conflict** iff the relation is *mutually* contradictory —
/// `Nli.classify(a -> b)` and `Nli.classify(b -> a)` both return
/// [`Entailment::Contradiction`] — and neither claim has been superseded
/// (its id is not present in `superseded_ids`). Identical-text pairs are skipped.
///
/// Mirrors [`crate::conflicts::detect::detect_conflicts`]'s [`ConflictEntry`]
/// output (deterministic pair id, `Open` status) so it can be swapped in later.
/// Pass the claims that should be considered superseded (e.g. the old side of the
/// edges from [`infer_supersessions_semantic`]) in `superseded_ids`.
pub fn detect_conflicts_semantic(
    claims: &[(ClaimId, ClaimView)],
    superseded_ids: &HashSet<ClaimId>,
    embedder: &dyn Embedder,
    nli: &dyn Nli,
    threshold: f32,
) -> Result<Vec<ConflictEntry>, PipelineError> {
    let groups = group_claims(claims, embedder, threshold)?;
    let mut conflicts: Vec<ConflictEntry> = Vec::new();

    for group in &groups {
        if group.len() < 2 {
            continue;
        }
        for (offset, &i) in group.iter().enumerate() {
            for &j in group.iter().skip(offset + 1) {
                let (a_id, a_view) = &claims[i];
                let (b_id, b_view) = &claims[j];
                if a_view.normalized_text == b_view.normalized_text {
                    continue;
                }
                if superseded_ids.contains(a_id) || superseded_ids.contains(b_id) {
                    continue;
                }
                let forward = nli.classify(embedding_text(a_view), embedding_text(b_view))?;
                if forward.label != Entailment::Contradiction {
                    continue;
                }
                let backward = nli.classify(embedding_text(b_view), embedding_text(a_view))?;
                if backward.label != Entailment::Contradiction {
                    continue;
                }
                conflicts.push(ConflictEntry {
                    conflict_id: conflict_id_from_pair(a_id, b_id),
                    claim_a: a_id.clone(),
                    claim_b: b_id.clone(),
                    subject_hint: a_view.subject_hint.clone(),
                    reason: format!(
                        "contradictory current claims: \"{}\" vs \"{}\"",
                        a_view.text, b_view.text
                    ),
                    status: ConflictStatus::Open,
                });
            }
        }
    }

    conflicts.sort_by(|x, y| x.conflict_id.as_str().cmp(y.conflict_id.as_str()));
    Ok(conflicts)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::extract::normalize::normalize_line;
    use crate::semantics::NliVerdict;
    use crate::types::ids::SourceId;
    use crate::types::receipt::receipt_view;
    use crate::types::status::ClaimStatus;

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

    /// Deterministic NLI driven by a scripted `(premise_sub, hypothesis_sub) ->
    /// label` table. The first entry whose substrings both match wins; unmatched
    /// pairs are [`Entailment::Neutral`] (the safe default — no supersession, no
    /// conflict). Keyed on distinctive phrases so wording is irrelevant.
    struct ScriptedNli {
        table: Vec<(&'static str, &'static str, Entailment)>,
    }

    impl ScriptedNli {
        fn new(table: Vec<(&'static str, &'static str, Entailment)>) -> Self {
            Self { table }
        }
    }

    impl Nli for ScriptedNli {
        fn classify(&self, premise: &str, hypothesis: &str) -> Result<NliVerdict, SemanticsError> {
            let p = premise.to_ascii_lowercase();
            let h = hypothesis.to_ascii_lowercase();
            for (premise_sub, hypothesis_sub, label) in &self.table {
                if p.contains(&premise_sub.to_ascii_lowercase())
                    && h.contains(&hypothesis_sub.to_ascii_lowercase())
                {
                    return Ok(NliVerdict {
                        label: *label,
                        score: 1.0,
                    });
                }
            }
            Ok(NliVerdict {
                label: Entailment::Neutral,
                score: 1.0,
            })
        }
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

    #[test]
    fn deploy_schedule_tuesday_supersedes_friday_and_wednesday_not_noise() {
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
        // Tuesday (newest) entails the earlier decisions; the noise question
        // entails nothing, so it must NOT supersede the decision.
        let nli = ScriptedNli::new(vec![
            ("tuesday", "friday", Entailment::Entailment),
            ("tuesday", "wednesday", Entailment::Entailment),
            ("wednesday", "friday", Entailment::Entailment),
        ]);
        let edges =
            infer_supersessions_semantic(&claims, &deploy_embedder(), &nli, 0.9).expect("infer");

        // Friday and Wednesday are superseded, each by Tuesday (the newest
        // entailing claim) — not a chain of intermediate edges.
        assert_eq!(
            edges.len(),
            2,
            "exactly Friday->Tuesday and Wednesday->Tuesday"
        );
        let pairs: Vec<(&str, &str)> = edges
            .iter()
            .map(|(o, n, _)| (o.as_str(), n.as_str()))
            .collect();
        assert!(
            pairs.contains(&("claim_aaaaaaaaaaaa", "claim_cccccccccccc")),
            "Friday->Tuesday"
        );
        assert!(
            pairs.contains(&("claim_bbbbbbbbbbbb", "claim_cccccccccccc")),
            "Wednesday->Tuesday"
        );
        // The noise claim is neither superseded nor a superseder.
        assert!(
            !pairs
                .iter()
                .any(|(o, n)| *o == "claim_dddddddddddd" || *n == "claim_dddddddddddd"),
            "noise question must not participate in supersession"
        );
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
    fn release_schedule_mutual_contradiction_is_a_conflict() {
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "Releases happen on Monday", 1),
            claim("claim_bbbbbbbbbbbb", "x", "Releases go out on Friday", 2),
            claim("claim_cccccccccccc", "x", "Bob owns release approval", 3),
        ];
        // Monday vs Friday mutually contradict; neither entails the other, so
        // neither is superseded -> genuine conflict. Approval is in its own group
        // and shares no NLI relation, so it is not dragged in.
        let nli = ScriptedNli::new(vec![
            ("monday", "friday", Entailment::Contradiction),
            ("friday", "monday", Entailment::Contradiction),
        ]);
        let conflicts =
            detect_conflicts_semantic(&claims, &HashSet::new(), &release_embedder(), &nli, 0.9)
                .expect("detect");
        assert_eq!(conflicts.len(), 1, "exactly one release-schedule conflict");
        let entry = &conflicts[0];
        let mut pair = [entry.claim_a.as_str(), entry.claim_b.as_str()];
        pair.sort_unstable();
        assert_eq!(pair, ["claim_aaaaaaaaaaaa", "claim_bbbbbbbbbbbb"]);
        assert_eq!(entry.status, ConflictStatus::Open);
        assert_eq!(
            entry.conflict_id,
            conflict_id_from_pair(&entry.claim_a, &entry.claim_b)
        );
    }

    #[test]
    fn one_sided_contradiction_is_not_a_conflict() {
        // Only the forward direction contradicts; the relation is not mutual, so
        // it must NOT be reported as a conflict.
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "Releases happen on Monday", 1),
            claim("claim_bbbbbbbbbbbb", "x", "Releases go out on Friday", 2),
        ];
        let nli = ScriptedNli::new(vec![("monday", "friday", Entailment::Contradiction)]);
        let embedder = FixedEmbedder::new(
            vec![
                ("releases happen on monday", vec![1.0, 0.0]),
                ("go out on friday", vec![0.95, 0.05]),
            ],
            2,
        );
        let conflicts = detect_conflicts_semantic(&claims, &HashSet::new(), &embedder, &nli, 0.9)
            .expect("detect");
        assert!(
            conflicts.is_empty(),
            "one-sided contradiction is not a conflict"
        );
    }

    #[test]
    fn superseded_claim_is_excluded_from_conflicts() {
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "Releases happen on Monday", 1),
            claim("claim_bbbbbbbbbbbb", "x", "Releases go out on Friday", 2),
        ];
        let nli = ScriptedNli::new(vec![
            ("monday", "friday", Entailment::Contradiction),
            ("friday", "monday", Entailment::Contradiction),
        ]);
        let embedder = FixedEmbedder::new(
            vec![
                ("releases happen on monday", vec![1.0, 0.0]),
                ("go out on friday", vec![0.95, 0.05]),
            ],
            2,
        );
        let mut superseded = HashSet::new();
        superseded.insert(ClaimId::try_from("claim_aaaaaaaaaaaa").expect("id"));
        let conflicts =
            detect_conflicts_semantic(&claims, &superseded, &embedder, &nli, 0.9).expect("detect");
        assert!(
            conflicts.is_empty(),
            "a superseded claim must not produce a conflict"
        );
    }

    #[test]
    fn storage_batpak_supersedes_postgres() {
        let claims = vec![
            claim(
                "claim_aaaaaaaaaaaa",
                "x",
                "Helios uses Postgres for storage",
                1,
            ),
            claim(
                "claim_bbbbbbbbbbbb",
                "x",
                "Helios uses BatPak for storage",
                2,
            ),
        ];
        let embedder = FixedEmbedder::new(
            vec![("postgres", vec![1.0, 0.0]), ("batpak", vec![0.96, 0.05])],
            2,
        );
        // BatPak (newer) is the current storage decision and entails/covers the
        // older Postgres claim — the semantic-reversal the keyword heuristic
        // missed.
        let nli = ScriptedNli::new(vec![("batpak", "postgres", Entailment::Entailment)]);
        let edges = infer_supersessions_semantic(&claims, &embedder, &nli, 0.9).expect("infer");
        assert_eq!(edges.len(), 1, "BatPak supersedes Postgres");
        let (old, new, reason) = &edges[0];
        assert_eq!(old.as_str(), "claim_aaaaaaaaaaaa");
        assert_eq!(new.as_str(), "claim_bbbbbbbbbbbb");
        assert!(reason.starts_with("superseded by"), "got {reason}");
    }

    #[test]
    fn empty_input_yields_no_groups_edges_or_conflicts() {
        let embedder = FixedEmbedder::new(Vec::new(), 2);
        let nli = ScriptedNli::new(Vec::new());
        assert!(group_claims(&[], &embedder, 0.9).expect("group").is_empty());
        assert!(infer_supersessions_semantic(&[], &embedder, &nli, 0.9)
            .expect("infer")
            .is_empty());
        assert!(
            detect_conflicts_semantic(&[], &HashSet::new(), &embedder, &nli, 0.9)
                .expect("detect")
                .is_empty()
        );
    }

    #[test]
    fn identical_text_pairs_are_skipped() {
        // Two claims with identical normalized text are duplicates: neither a
        // supersession nor a conflict, even when grouped and NLI would fire.
        let claims = vec![
            claim("claim_aaaaaaaaaaaa", "x", "Deploys moved to Tuesday", 1),
            claim("claim_bbbbbbbbbbbb", "x", "Deploys moved to Tuesday", 2),
        ];
        let embedder = FixedEmbedder::new(vec![("tuesday", vec![1.0, 0.0])], 2);
        let nli = ScriptedNli::new(vec![("tuesday", "tuesday", Entailment::Entailment)]);
        let edges = infer_supersessions_semantic(&claims, &embedder, &nli, 0.9).expect("infer");
        assert!(edges.is_empty(), "duplicate text is not a supersession");
        let conflicts = detect_conflicts_semantic(&claims, &HashSet::new(), &embedder, &nli, 0.9)
            .expect("detect");
        assert!(conflicts.is_empty(), "duplicate text is not a conflict");
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
}
