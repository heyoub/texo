//! Backend-local error types for the semantic implementations.
//!
//! Every variant carries a typed `#[source]`/`#[from]` cause and is converted
//! into [`texo_core::SemanticsError::Backend`] at the trait boundary, so callers
//! see one structured error surface regardless of which runtime failed.
//!
//! Variants are feature-gated to match the backend that raises them: the
//! OpenRouter HTTP variants exist under `openrouter`, the ONNX variants under
//! `local-onnx`. A handful of shape/output variants are shared.

#[cfg(feature = "local-onnx")]
use std::path::PathBuf;

/// Failures from the semantic backends.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BackendError {
    // --- OpenRouter (hosted HTTP) backend ---
    /// No API key was available: neither an explicit argument nor the
    /// `OPENROUTER_API_KEY` environment variable was set.
    #[cfg(feature = "openrouter")]
    #[error("missing OpenRouter API key (set OPENROUTER_API_KEY or pass one explicitly)")]
    MissingApiKey,
    /// The HTTP client could not be constructed.
    #[cfg(feature = "openrouter")]
    #[error("failed to build the OpenRouter HTTP client")]
    HttpClientBuild {
        /// The typed cause from reqwest.
        #[source]
        source: reqwest::Error,
    },
    /// An HTTP request to OpenRouter failed at the transport level (DNS,
    /// connection, timeout) or could not be sent.
    #[cfg(feature = "openrouter")]
    #[error("OpenRouter request to {endpoint} failed")]
    Http {
        /// The endpoint path that was requested (e.g. `/embeddings`).
        endpoint: &'static str,
        /// The typed cause from reqwest.
        #[source]
        source: reqwest::Error,
    },
    /// OpenRouter returned a non-success HTTP status, including after retries.
    #[cfg(feature = "openrouter")]
    #[error("OpenRouter {endpoint} returned HTTP {status}: {body}")]
    HttpStatus {
        /// The endpoint path that was requested.
        endpoint: &'static str,
        /// The HTTP status code returned.
        status: u16,
        /// A truncated copy of the response body for diagnostics.
        body: String,
    },
    /// An OpenRouter response body could not be parsed as the expected JSON
    /// shape.
    #[cfg(feature = "openrouter")]
    #[error("could not parse OpenRouter {endpoint} response")]
    Parse {
        /// The endpoint path whose response failed to parse.
        endpoint: &'static str,
        /// The typed JSON cause.
        #[source]
        source: serde_json::Error,
    },
    /// An OpenRouter response parsed as JSON but lacked the fields this backend
    /// requires (e.g. an empty `data` array, or a chat reply that was not the
    /// strict NLI JSON the prompt asked for).
    #[cfg(feature = "openrouter")]
    #[error("OpenRouter {endpoint} response was missing required data: {detail}")]
    UnexpectedResponse {
        /// The endpoint path whose response was malformed.
        endpoint: &'static str,
        /// Human-readable detail about what was missing or invalid.
        detail: String,
    },

    // --- Local ONNX backend ---
    /// The fastembed text-embedding runtime failed to initialise or embed.
    #[cfg(feature = "local-onnx")]
    #[error("embedding backend failed")]
    Embedding {
        /// The typed cause from fastembed.
        #[source]
        source: fastembed::Error,
    },
    /// The fastembed backend returned no embedding for an input it was given.
    #[cfg(feature = "local-onnx")]
    #[error("embedding backend returned no vector for the input text")]
    EmptyEmbedding,
    /// A model artifact could not be fetched from the Hugging Face Hub.
    #[cfg(feature = "local-onnx")]
    #[error("failed to download model artifact from the Hugging Face Hub")]
    Download {
        /// The typed cause from hf-hub.
        #[source]
        source: hf_hub::api::sync::ApiError,
    },
    /// The tokenizer file could not be loaded or applied.
    #[cfg(feature = "local-onnx")]
    #[error("tokenizer failure")]
    Tokenizer {
        /// The boxed cause from the tokenizers crate.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    /// The ONNX Runtime session failed to build or run.
    #[cfg(feature = "local-onnx")]
    #[error("onnx runtime session failure")]
    Onnx {
        /// The typed cause from ort.
        #[source]
        source: ort::Error,
    },
    /// A model `config.json` could not be read or parsed.
    #[cfg(feature = "local-onnx")]
    #[error("could not read model config at {path}")]
    Config {
        /// Path that was read.
        path: PathBuf,
        /// The typed cause (I/O or JSON parsing).
        #[source]
        source: ConfigError,
    },
    /// The tokenized input was too long to represent as an ONNX tensor shape.
    #[cfg(feature = "local-onnx")]
    #[error("tokenized input length {len} exceeds the supported tensor range")]
    InputTooLong {
        /// The offending token count.
        len: usize,
    },
    /// The model produced output of an unexpected shape (e.g. empty logits).
    #[cfg(feature = "local-onnx")]
    #[error("model produced no usable output (expected non-empty logits)")]
    EmptyOutput,
    /// The model config's `id2label` map referenced an index the logits row did
    /// not contain, or a label string the NLI scheme does not recognise.
    #[cfg(feature = "local-onnx")]
    #[error("model label map is inconsistent with the model output")]
    LabelMapMismatch,
}

/// Causes for [`BackendError::Config`].
#[cfg(feature = "local-onnx")]
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The config file could not be read from disk.
    #[error("reading config file")]
    Io(#[from] std::io::Error),
    /// The config file was not valid JSON or lacked the expected fields.
    #[error("parsing config json")]
    Parse(#[from] serde_json::Error),
    /// The config JSON parsed but lacked a usable `id2label` map.
    #[error("config is missing a usable id2label map")]
    MissingId2Label,
}

impl From<BackendError> for texo_core::SemanticsError {
    fn from(err: BackendError) -> Self {
        texo_core::SemanticsError::Backend {
            source: Box::new(err),
        }
    }
}
