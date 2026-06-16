//! Semantic backends for `texo-core`.
//!
//! This crate is the runtime side of `texo-core`'s pure semantic trait seam. It
//! ships two interchangeable implementations of [`texo_core::Embedder`],
//! [`texo_core::Reranker`], and [`texo_core::Nli`], selected by Cargo feature:
//!
//! - **`openrouter`** (default): hosted HTTP backends that call the OpenRouter
//!   API over `reqwest::blocking`. These run on any CPU and need only an API
//!   key, making them the portable default. See [`OpenRouterEmbedder`],
//!   [`OpenRouterReranker`], and [`OpenRouterNli`].
//! - **`local-onnx`** (opt-in): local models executed via ONNX Runtime
//!   (fastembed + `ort`). These require AVX2-class hardware and download model
//!   artifacts on first use. See [`LocalEmbedder`] and [`LocalNli`].
//!
//! All backend, transport, and parsing failures are mapped to
//! [`texo_core::SemanticsError`] at the trait boundary, so callers see one
//! structured error surface regardless of which backend is enabled.

mod error;

#[cfg(feature = "local-onnx")]
mod embedder;
#[cfg(feature = "local-onnx")]
mod nli;
#[cfg(feature = "openrouter")]
mod openrouter;

pub use error::BackendError;

#[cfg(feature = "local-onnx")]
pub use error::ConfigError;

#[cfg(feature = "local-onnx")]
pub use embedder::LocalEmbedder;
#[cfg(feature = "local-onnx")]
pub use nli::LocalNli;

#[cfg(feature = "openrouter")]
pub use openrouter::{OpenRouterEmbedder, OpenRouterNli, OpenRouterReranker};

#[cfg(all(test, feature = "local-onnx"))]
mod local_smoke {
    //! Live de-risk smoke tests for the local ONNX backends. These download
    //! models from the Hugging Face Hub and run real inference, so they are
    //! `#[ignore]`d by default and run explicitly with
    //! `cargo test -p texo-semantics --features local-onnx -- --ignored --nocapture`.

    use texo_core::{cosine_similarity, Embedder, Entailment, Nli};

    use crate::{LocalEmbedder, LocalNli};

    #[test]
    #[ignore = "downloads a model from the Hugging Face Hub; run explicitly"]
    fn embedder_paraphrase_beats_unrelated() {
        let embedder = LocalEmbedder::new().expect("build embedder");
        let base = embedder
            .embed("Deploys happen on Friday")
            .expect("embed base");
        let paraphrase = embedder
            .embed("The deploy schedule is Friday")
            .expect("embed paraphrase");
        let unrelated = embedder.embed("Lunch was tacos").expect("embed unrelated");

        let para_sim = cosine_similarity(&base, &paraphrase);
        let unrel_sim = cosine_similarity(&base, &unrelated);

        println!("paraphrase cosine = {para_sim:.4}");
        println!("unrelated  cosine = {unrel_sim:.4}");

        assert!(
            para_sim > unrel_sim + 0.15,
            "paraphrase cosine ({para_sim:.4}) should clearly exceed unrelated ({unrel_sim:.4})"
        );
    }

    #[test]
    #[ignore = "downloads a model from the Hugging Face Hub; run explicitly"]
    fn nli_classifies_contradiction_and_entailment() {
        let nli = LocalNli::new().expect("build nli");

        let contra = nli
            .classify("Deploys moved to Tuesday.", "Deploys happen on Friday.")
            .expect("classify contradiction");
        println!(
            "contradiction case -> {:?} ({:.4})",
            contra.label, contra.score
        );

        let entail = nli
            .classify(
                "The team uses BatPak for storage now.",
                "The platform uses BatPak.",
            )
            .expect("classify entailment");
        println!(
            "entailment case -> {:?} ({:.4})",
            entail.label, entail.score
        );

        assert_eq!(contra.label, Entailment::Contradiction);
        assert_eq!(entail.label, Entailment::Entailment);
    }
}
