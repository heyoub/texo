use super::*;

use crate::extract::normalize::normalize_line;
use crate::semantics::ClaimRelater;

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

fn complete(outcome: &RelateOutcome) -> Option<&CompleteRelateOutcome> {
    outcome.complete()
}

fn partial(outcome: &RelateOutcome) -> Option<&PartialRelateOutcome> {
    outcome.partial()
}

fn related(outcome: &RelateOutcome) -> Option<&RelatedClaims> {
    outcome.complete().map(|complete| &complete.related)
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

/// Deterministic relater that counts every judge call and always answers
/// [`ClaimRelation::Unrelated`].
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

struct FingerprintRelater {
    inner: CountingRelater,
    fingerprint: &'static str,
}

impl ClaimRelater for FingerprintRelater {
    fn relate(&self, older: &str, newer: &str) -> Result<RelationVerdict, SemanticsError> {
        self.inner.relate(older, newer)
    }

    fn fingerprint(&self) -> String {
        self.fingerprint.to_string()
    }
}

// Corpus of `k = 3` well-separated clusters of size `m = 3` (n = 9), as 2-D
// unit vectors at angular offsets: within-cluster spread ≤ 2° (cosine
// ≥ 0.999), adjacent clusters 30° apart (cosine ≈ 0.85–0.88 — above the 0.6
// prefilter, below the 0.98 cluster threshold), far clusters 60° apart
// (cosine < 0.6).

mod basic;
mod settlement;
mod parallel;
mod temporal;
