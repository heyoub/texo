//! Local text-embedding backend built on fastembed's ONNX runtime.

use std::sync::Mutex;

use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use texo_core::{Embedder, SemanticsError};

use crate::error::BackendError;

/// The embedding model this backend pins to: BGE-small English v1.5 (384-dim).
///
/// Pinned deliberately so embedding geometry is stable across runs; callers
/// compare vectors via [`texo_core::cosine_similarity`] and must not mix vectors
/// produced by different models.
const PINNED_MODEL: EmbeddingModel = EmbeddingModel::BGESmallENV15;

/// Local [`Embedder`] backed by fastembed's `TextEmbedding`.
///
/// The underlying fastembed handle requires `&mut self` to embed, while the
/// [`Embedder`] trait is `&self`; the handle is therefore held behind a
/// [`Mutex`] so a shared `LocalEmbedder` can serve embed calls.
pub struct LocalEmbedder {
    inner: Mutex<TextEmbedding>,
}

impl LocalEmbedder {
    /// Build a `LocalEmbedder`, downloading the pinned BGE model on first use.
    ///
    /// ONNX intra-op threads are capped at one for deterministic, low-variance
    /// inference.
    pub fn new() -> Result<Self, SemanticsError> {
        let options = TextInitOptions::new(PINNED_MODEL)
            .with_show_download_progress(false)
            .with_intra_threads(1);
        let model =
            TextEmbedding::try_new(options).map_err(|source| BackendError::Embedding { source })?;
        Ok(Self {
            inner: Mutex::new(model),
        })
    }
}

impl Embedder for LocalEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, SemanticsError> {
        let mut vectors = self.embed_batch(&[text])?;
        // `embed_batch` returns exactly one vector per input; pop it.
        vectors.pop().ok_or(SemanticsError::ResultCountMismatch {
            expected: 1,
            actual: 0,
        })
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SemanticsError> {
        // A poisoned lock means a prior embed panicked inside fastembed; surface
        // it as a structured backend error rather than propagating the panic.
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| BackendError::EmptyEmbedding)?;
        let owned: Vec<String> = texts.iter().map(|t| (*t).to_owned()).collect();
        let vectors = guard
            .embed(&owned, None)
            .map_err(|source| BackendError::Embedding { source })?;
        if vectors.len() != texts.len() {
            return Err(SemanticsError::ResultCountMismatch {
                expected: texts.len(),
                actual: vectors.len(),
            });
        }
        Ok(vectors)
    }
}
