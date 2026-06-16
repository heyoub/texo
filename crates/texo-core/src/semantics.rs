//! Semantic-pipeline trait seam.
//!
//! This module defines the pure trait boundary that a later ML pipeline plugs
//! into. It carries **no** ML, HTTP, or model-runtime dependencies: only trait
//! definitions, plain data types, and a deterministic cosine helper. Default
//! behavior of texo is unchanged — every semantic capability is opt-in via an
//! implementor supplied by a higher layer.

use serde::{Deserialize, Serialize};

/// Maps text into a dense embedding vector.
///
/// Implementors decide the embedding dimensionality; callers must not assume a
/// fixed width and should compare vectors only via [`cosine_similarity`], which
/// degrades gracefully on length mismatch.
pub trait Embedder {
    /// Embed a single piece of text.
    fn embed(&self, text: &str) -> Result<Vec<f32>, SemanticsError>;

    /// Embed a batch of texts.
    ///
    /// The default implementation loops over [`Embedder::embed`]; implementors
    /// backed by a batching runtime should override this for efficiency.
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SemanticsError> {
        texts.iter().map(|text| self.embed(text)).collect()
    }
}

/// Natural-language-inference relationship between a premise and a hypothesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entailment {
    /// The premise supports (entails) the hypothesis.
    Entailment,
    /// The premise neither supports nor contradicts the hypothesis.
    Neutral,
    /// The premise contradicts the hypothesis.
    Contradiction,
}

/// A single NLI classification result.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NliVerdict {
    /// Predicted relationship.
    pub label: Entailment,
    /// Confidence score for [`NliVerdict::label`], typically in `[0.0, 1.0]`.
    pub score: f32,
}

/// Classifies the inference relationship between two texts.
pub trait Nli {
    /// Classify whether `premise` entails, is neutral toward, or contradicts
    /// `hypothesis`.
    fn classify(&self, premise: &str, hypothesis: &str) -> Result<NliVerdict, SemanticsError>;
}

/// The relationship of a *newer* claim to an *older* one about a shared subject.
///
/// This is the judgment a 3-way NLI label cannot make: a value replacement (e.g.
/// "deploys moved to Tuesday" vs "deploys happen on Friday") and a genuine
/// disagreement (e.g. two docs asserting different release days) are *both*
/// mutual contradictions at the NLI level. Separating them requires reasoning
/// about recency and update-intent — and about whether the two claims even share
/// a subject ("Friday deploy" vs "Friday release" embed almost identically). A
/// [`ClaimRelater`] makes that single, richer call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimRelation {
    /// Same subject; the newer claim updates/replaces the older's value (the fact
    /// changed over time). The newer **supersedes** the older.
    Supersedes,
    /// Same subject; incompatible values with no sign one updates the other — a
    /// genuine **conflict** between claims of equal standing.
    Conflict,
    /// The two claims state the same fact.
    Duplicate,
    /// Different subjects/attributes, or compatible and independent. Claims that
    /// merely share a token (a weekday, the word "release") but concern different
    /// subjects are unrelated.
    Unrelated,
}

/// A single claim-relation judgment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RelationVerdict {
    /// The judged relation of the newer claim to the older.
    pub relation: ClaimRelation,
    /// Confidence score for [`RelationVerdict::relation`], typically in `[0, 1]`.
    pub score: f32,
}

/// Judges the relationship between two claims, with recency known to the caller.
///
/// The caller orders the pair by recency and passes them as `(older, newer)`; the
/// implementor returns how `newer` relates to `older` (see [`ClaimRelation`]).
/// This is the primary relating primitive of the semantic pipeline; [`Nli`]
/// remains available as a lower-level building block.
pub trait ClaimRelater {
    /// Judge how the more-recent `newer` claim relates to the older `older` claim.
    fn relate(&self, older: &str, newer: &str) -> Result<RelationVerdict, SemanticsError>;
}

/// Scores candidate documents against a query for relevance reranking.
pub trait Reranker {
    /// Return one relevance score per entry in `docs`, in the same order.
    ///
    /// Implementors must return exactly `docs.len()` scores.
    fn rerank(&self, query: &str, docs: &[&str]) -> Result<Vec<f32>, SemanticsError>;
}

/// Cosine similarity between two embedding vectors.
///
/// This is deterministic and never panics. It returns `0.0` when the vectors
/// have **mismatched or zero length**, or when either vector has a zero norm,
/// rather than dividing by zero or indexing out of bounds. A returned `0.0`
/// therefore means "no usable signal," which is the safe neutral value for
/// downstream thresholding.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (&x, &y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

/// Failures raised by semantic-pipeline backends.
#[derive(Debug, thiserror::Error)]
pub enum SemanticsError {
    /// Two vectors that were required to share a dimensionality did not.
    #[error("embedding dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch {
        /// Expected vector length.
        expected: usize,
        /// Observed vector length.
        actual: usize,
    },
    /// The backend produced a result with an unexpected shape (e.g. a reranker
    /// returning a different number of scores than the documents it was given).
    #[error("backend returned {actual} results for {expected} inputs")]
    ResultCountMismatch {
        /// Expected number of results.
        expected: usize,
        /// Observed number of results.
        actual: usize,
    },
    /// The underlying model/runtime backend failed, carrying its typed cause.
    #[error("semantics backend failure")]
    Backend {
        /// The typed source error from the backend.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic in-test embedder: folds bytes into a fixed-width vector.
    /// Not a model stand-in for behavior, only to exercise the trait seam.
    struct ConstantEmbedder {
        width: usize,
    }

    impl Embedder for ConstantEmbedder {
        fn embed(&self, text: &str) -> Result<Vec<f32>, SemanticsError> {
            let mut out = vec![0.0f32; self.width];
            for (i, byte) in text.bytes().enumerate() {
                out[i % self.width] += f32::from(byte);
            }
            Ok(out)
        }
    }

    /// Deterministic NLI stub: equal strings entail, otherwise neutral.
    struct StubNli;

    impl Nli for StubNli {
        fn classify(&self, premise: &str, hypothesis: &str) -> Result<NliVerdict, SemanticsError> {
            let label = if premise == hypothesis {
                Entailment::Entailment
            } else {
                Entailment::Neutral
            };
            Ok(NliVerdict { label, score: 1.0 })
        }
    }

    #[test]
    fn cosine_orthogonal_is_zero() {
        let a = [1.0, 0.0];
        let b = [0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn cosine_identical_is_one() {
        let a = [0.3, 0.5, 0.9];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
    }

    /// Assert a float is the exact `0.0` neutral value via bit comparison,
    /// which is precise and avoids clippy's strict-float-comparison lint.
    fn is_exact_zero(value: f32) -> bool {
        value.to_bits() == 0.0f32.to_bits()
    }

    #[test]
    fn cosine_zero_norm_and_empty_return_zero_without_panic() {
        let zero = [0.0, 0.0, 0.0];
        let v = [1.0, 2.0, 3.0];
        assert!(is_exact_zero(cosine_similarity(&zero, &v)));
        assert!(is_exact_zero(cosine_similarity(&v, &zero)));
        let empty: [f32; 0] = [];
        assert!(is_exact_zero(cosine_similarity(&empty, &empty)));
    }

    #[test]
    fn cosine_length_mismatch_returns_zero() {
        let a = [1.0, 2.0, 3.0];
        let b = [1.0, 2.0];
        assert!(is_exact_zero(cosine_similarity(&a, &b)));
    }

    #[test]
    fn cosine_known_vector_value() {
        // a = (1,2,3), b = (4,5,6): dot=32, |a|=sqrt(14), |b|=sqrt(77).
        let a = [1.0f32, 2.0, 3.0];
        let b = [4.0f32, 5.0, 6.0];
        let expected = 32.0f32 / (14.0f32.sqrt() * 77.0f32.sqrt());
        assert!((cosine_similarity(&a, &b) - expected).abs() < 1e-6);
    }

    #[test]
    fn embedder_usable_as_trait_object_with_default_batch() {
        let embedder = ConstantEmbedder { width: 4 };
        let dynamic: &dyn Embedder = &embedder;
        let single = dynamic.embed("hi").expect("embed");
        assert_eq!(single.len(), 4);

        let texts = ["hi", "there"];
        let batch = dynamic
            .embed_batch(&texts)
            .expect("default embed_batch loops embed");
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0], single);
    }

    #[test]
    fn nli_usable_as_trait_object() {
        let nli = StubNli;
        let dynamic: &dyn Nli = &nli;
        let same = dynamic.classify("a", "a").expect("classify");
        assert_eq!(same.label, Entailment::Entailment);
        let diff = dynamic.classify("a", "b").expect("classify");
        assert_eq!(diff.label, Entailment::Neutral);
    }

    /// Deterministic relater stub: equal strings duplicate, otherwise unrelated.
    struct StubRelater;

    impl ClaimRelater for StubRelater {
        fn relate(&self, older: &str, newer: &str) -> Result<RelationVerdict, SemanticsError> {
            let relation = if older == newer {
                ClaimRelation::Duplicate
            } else {
                ClaimRelation::Unrelated
            };
            Ok(RelationVerdict {
                relation,
                score: 1.0,
            })
        }
    }

    #[test]
    fn relater_usable_as_trait_object() {
        let relater = StubRelater;
        let dynamic: &dyn ClaimRelater = &relater;
        let same = dynamic.relate("a", "a").expect("relate");
        assert_eq!(same.relation, ClaimRelation::Duplicate);
        let diff = dynamic.relate("a", "b").expect("relate");
        assert_eq!(diff.relation, ClaimRelation::Unrelated);
    }

    #[test]
    fn claim_relation_serde_roundtrip() {
        let json = serde_json::to_string(&ClaimRelation::Supersedes).expect("ser");
        assert_eq!(json, "\"supersedes\"");
        let back: ClaimRelation = serde_json::from_str(&json).expect("de");
        assert_eq!(back, ClaimRelation::Supersedes);
        // Snake-case for the multi-word-free variants too.
        assert_eq!(
            serde_json::to_string(&ClaimRelation::Conflict).expect("ser"),
            "\"conflict\""
        );
    }

    #[test]
    fn entailment_serde_roundtrip() {
        let json = serde_json::to_string(&Entailment::Contradiction).expect("ser");
        assert_eq!(json, "\"contradiction\"");
        let back: Entailment = serde_json::from_str(&json).expect("de");
        assert_eq!(back, Entailment::Contradiction);
    }
}
