//! Local natural-language-inference backend built directly on ONNX Runtime.
//!
//! The model is a pre-exported cross-encoder DeBERTa-v3 MNLI head. Crucially,
//! the mapping from output index to entailment label is read from the model's
//! `config.json` `id2label` map at load time rather than hardcoded: label order
//! differs across model families (this cross-encoder uses
//! `0=contradiction, 1=entailment, 2=neutral`, which is not the canonical MNLI
//! order), so hardcoding would silently mislabel verdicts.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use texo_core::{Entailment, Nli, NliVerdict, SemanticsError};
use tokenizers::Tokenizer;

use crate::error::{BackendError, ConfigError};

/// Hugging Face repo for the pinned NLI model.
const MODEL_REPO: &str = "cross-encoder/nli-deberta-v3-small";
/// ONNX graph file within the repo. This export takes only `input_ids` and
/// `attention_mask` (DeBERTa-v3 does not use token-type ids) and emits `logits`.
const MODEL_FILE: &str = "onnx/model.onnx";
/// Tokenizer file within the repo.
const TOKENIZER_FILE: &str = "tokenizer.json";
/// Model config carrying the `id2label` map.
const CONFIG_FILE: &str = "config.json";

/// Local [`Nli`] backed by an ONNX DeBERTa-v3 MNLI cross-encoder.
///
/// The ONNX session requires `&mut self` to run, while [`Nli`] is `&self`; the
/// session is held behind a [`Mutex`] for shared use.
pub struct LocalNli {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    /// Map from logit index to the entailment label, derived from `id2label`.
    index_to_label: Vec<Entailment>,
}

impl LocalNli {
    /// Build a `LocalNli`, downloading the model, tokenizer, and config on first
    /// use. The ONNX session runs single-threaded (one intra-op and one inter-op
    /// thread) for deterministic inference.
    pub fn new() -> Result<Self, SemanticsError> {
        let api = hf_hub::api::sync::ApiBuilder::new()
            .with_progress(false)
            .build()
            .map_err(|source| BackendError::Download { source })?;
        let repo = api.model(MODEL_REPO.to_owned());

        let model_path = repo
            .get(MODEL_FILE)
            .map_err(|source| BackendError::Download { source })?;
        let tokenizer_path = repo
            .get(TOKENIZER_FILE)
            .map_err(|source| BackendError::Download { source })?;
        let config_path = repo
            .get(CONFIG_FILE)
            .map_err(|source| BackendError::Download { source })?;

        let index_to_label = load_label_map(&config_path)?;

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|source| BackendError::Tokenizer { source })?;

        let session = build_session(&model_path)?;

        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            index_to_label,
        })
    }
}

impl Nli for LocalNli {
    fn classify(&self, premise: &str, hypothesis: &str) -> Result<NliVerdict, SemanticsError> {
        // Cross-encoder NLI: the pair (premise, hypothesis) is encoded jointly.
        let encoding = self
            .tokenizer
            .encode((premise, hypothesis), true)
            .map_err(|source| BackendError::Tokenizer { source })?;

        let ids: Vec<i64> = encoding.get_ids().iter().map(|&id| i64::from(id)).collect();
        let mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&m| i64::from(m))
            .collect();
        let seq_len =
            i64::try_from(ids.len()).map_err(|_| BackendError::InputTooLong { len: ids.len() })?;
        let shape = [1_i64, seq_len];

        let input_ids =
            Tensor::from_array((shape, ids)).map_err(|source| BackendError::Onnx { source })?;
        let attention_mask =
            Tensor::from_array((shape, mask)).map_err(|source| BackendError::Onnx { source })?;

        let logits = {
            let mut guard = self.session.lock().map_err(|_| BackendError::EmptyOutput)?;
            let outputs = guard
                .run(ort::inputs![
                    "input_ids" => input_ids,
                    "attention_mask" => attention_mask,
                ])
                .map_err(|source| BackendError::Onnx { source })?;
            let (_, data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|source| BackendError::Onnx { source })?;
            // The output is [1, num_labels]; copy the single row out of the
            // borrow so the session lock can be released.
            data.to_vec()
        };

        if logits.is_empty() || logits.len() != self.index_to_label.len() {
            return Err(BackendError::EmptyOutput.into());
        }

        let probabilities = softmax(&logits);
        let (best_index, &best_score) = probabilities
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .ok_or(BackendError::EmptyOutput)?;
        let label = *self
            .index_to_label
            .get(best_index)
            .ok_or(BackendError::LabelMapMismatch)?;

        Ok(NliVerdict {
            label,
            score: best_score,
        })
    }
}

/// Build a single-threaded ONNX session from a model file. The session-builder
/// methods return `ort::Error<SessionBuilder>`; `?` converts each into the plain
/// `ort::Error` this returns, keeping the chain readable.
fn build_session(model_path: &Path) -> Result<Session, BackendError> {
    let build = || -> Result<Session, ort::Error> {
        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_intra_threads(1)?
            .with_inter_threads(1)?
            .commit_from_file(model_path)?;
        Ok(session)
    };
    build().map_err(|source| BackendError::Onnx { source })
}

/// Read the `id2label` map from a model `config.json` and translate it into a
/// dense `index -> Entailment` table. Returns an error rather than guessing if
/// the map is missing or names a label outside the NLI scheme.
fn load_label_map(config_path: &Path) -> Result<Vec<Entailment>, BackendError> {
    let bytes = std::fs::read(config_path).map_err(|e| BackendError::Config {
        path: config_path.to_owned(),
        source: ConfigError::Io(e),
    })?;
    let config: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| BackendError::Config {
            path: config_path.to_owned(),
            source: ConfigError::Parse(e),
        })?;

    let id2label = config
        .get("id2label")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| BackendError::Config {
            path: config_path.to_owned(),
            source: ConfigError::MissingId2Label,
        })?;

    // Collect into an index-keyed map first so we can build a dense, contiguous
    // table and reject gaps.
    let mut by_index: HashMap<usize, Entailment> = HashMap::with_capacity(id2label.len());
    for (key, value) in id2label {
        let index: usize = key.parse().map_err(|_| BackendError::Config {
            path: config_path.to_owned(),
            source: ConfigError::MissingId2Label,
        })?;
        let label_str = value.as_str().ok_or_else(|| BackendError::Config {
            path: config_path.to_owned(),
            source: ConfigError::MissingId2Label,
        })?;
        let label = parse_label(label_str).ok_or(BackendError::LabelMapMismatch)?;
        by_index.insert(index, label);
    }

    let mut table = Vec::with_capacity(by_index.len());
    for index in 0..by_index.len() {
        let label = by_index
            .get(&index)
            .copied()
            .ok_or(BackendError::LabelMapMismatch)?;
        table.push(label);
    }
    if table.is_empty() {
        return Err(BackendError::Config {
            path: config_path.to_owned(),
            source: ConfigError::MissingId2Label,
        });
    }
    Ok(table)
}

/// Map a model label string onto the [`Entailment`] scheme, tolerant of casing
/// and the common short/long spellings used across NLI model families.
fn parse_label(raw: &str) -> Option<Entailment> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "entailment" | "entail" => Some(Entailment::Entailment),
        "neutral" => Some(Entailment::Neutral),
        "contradiction" | "contradict" => Some(Entailment::Contradiction),
        _ => None,
    }
}

/// Numerically stable softmax over a logit row.
fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|&x| (x - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    if sum == 0.0 {
        return vec![0.0; logits.len()];
    }
    exps.into_iter().map(|e| e / sum).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn softmax_sums_to_one() {
        let p = softmax(&[1.0, 2.0, 3.0]);
        let sum: f32 = p.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
        // Largest logit yields largest probability.
        assert!(p[2] > p[1] && p[1] > p[0]);
    }

    #[test]
    fn parse_label_handles_known_spellings() {
        assert_eq!(parse_label("ENTAILMENT"), Some(Entailment::Entailment));
        assert_eq!(parse_label(" neutral "), Some(Entailment::Neutral));
        assert_eq!(
            parse_label("contradiction"),
            Some(Entailment::Contradiction)
        );
        assert_eq!(parse_label("nonsense"), None);
    }
}
