//! Claim extraction heuristics.

pub mod heuristics;
pub mod hints;
pub mod normalize;

pub use heuristics::is_claim_line;
pub use hints::ClaimHints;
pub use normalize::normalize_line;

use crate::events::payloads::ClaimRecorded;
use crate::source::markdown::MarkdownDocument;
use crate::types::ids::{claim_id_from_parts, SourceId};

/// One extracted claim prior to journaling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedClaim {
    /// Claim payload ready for append.
    pub payload: ClaimRecorded,
}

/// Extract claims from a parsed markdown document.
pub fn extract_claims(
    doc: &MarkdownDocument,
    source_id: &SourceId,
    workspace_id: &str,
    observed_at_ms: u64,
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
                extractor_kind: "heuristic-v0".to_string(),
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
}
