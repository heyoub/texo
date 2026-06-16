//! Claim extraction heuristics.

pub mod cmd;
pub mod heuristics;
pub mod hints;
pub mod normalize;
pub mod word_match;

pub use cmd::extract_via_cmd;
pub use heuristics::is_claim_line;
pub use hints::ClaimHints;
pub use normalize::normalize_line;

use crate::events::payloads::ClaimRecorded;
use crate::source::markdown::MarkdownDocument;
use crate::types::ids::{claim_id_from_parts, SourceId};

/// Extractor version tag written to journaled claims.
pub const EXTRACTOR_KIND_HEURISTIC_V1: &str = "heuristic-v1";

/// Default `confidence_ppm` for a claim whose confidence is unspecified.
///
/// Applied both by the heuristic extractor when no confidence-bearing keyword is
/// found, and by the external-command adapter when a JSON line omits
/// `confidence_ppm`. A single conservative default keeps the two extraction
/// paths in agreement: an unspecified confidence should not be inflated.
pub const DEFAULT_CONFIDENCE_PPM: u32 = 500_000;

/// Function pointer type for compositional ingest extraction.
pub type ExtractClaimsFn =
    fn(&MarkdownDocument, &SourceId, &str, u64) -> Result<Vec<ExtractedClaim>, ExtractError>;

/// One extracted claim prior to journaling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedClaim {
    /// Claim payload ready for append.
    pub payload: ClaimRecorded,
}

/// Extract claims from a parsed markdown document using heuristic-v1 rules.
pub fn extract_claims(
    doc: &MarkdownDocument,
    source_id: &SourceId,
    workspace_id: &str,
    observed_at_ms: u64,
) -> Result<Vec<ExtractedClaim>, ExtractError> {
    extract_with_kind(
        doc,
        source_id,
        workspace_id,
        observed_at_ms,
        EXTRACTOR_KIND_HEURISTIC_V1,
    )
}

/// Extract claims with an explicit extractor kind tag.
pub fn extract_with_kind(
    doc: &MarkdownDocument,
    source_id: &SourceId,
    workspace_id: &str,
    observed_at_ms: u64,
    extractor_kind: &str,
) -> Result<Vec<ExtractedClaim>, ExtractError> {
    let mut claims = Vec::new();
    for line in &doc.lines {
        if !heuristics::is_claim_line(&line.text) {
            continue;
        }
        let Some(hints) = hints::hints_from_line(&line.text) else {
            continue;
        };
        let normalized = normalize::normalize_line(&line.text);
        let claim_id = claim_id_from_parts(source_id, line.number, &normalized);
        claims.push(ExtractedClaim {
            payload: ClaimRecorded {
                claim_id: claim_id.to_string(),
                workspace_id: workspace_id.to_string(),
                source_id: source_id.to_string(),
                source_path: doc.path.clone(),
                line_start: line.number,
                line_end: line.number,
                text: line.text.clone(),
                normalized_text: normalized,
                subject_hint: hints.subject_hint,
                predicate_hint: hints.predicate_hint,
                object_hint: hints.object_hint,
                confidence_ppm: hints.confidence_ppm,
                extractor_kind: extractor_kind.to_string(),
                observed_at_ms,
            },
        });
    }
    Ok(claims)
}

/// Extraction failures.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    /// Source parse error.
    #[error("source: {0}")]
    Source(#[from] crate::source::SourceError),
    /// Domain validation error.
    #[error("{0}")]
    Domain(String),
    /// External extractor command failed.
    #[error("extractor cmd: {0}")]
    Cmd(String),
    /// Spawning or driving the external extractor process failed.
    #[error("extractor cmd: {context}: {source}")]
    CmdIo {
        /// What the extractor adapter was doing when the I/O error occurred.
        context: &'static str,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// The extractor's stdout was not valid UTF-8.
    #[error("extractor cmd: stdout not utf-8: {0}")]
    CmdUtf8(#[from] std::str::Utf8Error),
    /// A JSON line emitted by the extractor could not be parsed.
    #[error("extractor cmd: invalid json line: {0}")]
    CmdJson(#[from] serde_json::Error),
}
