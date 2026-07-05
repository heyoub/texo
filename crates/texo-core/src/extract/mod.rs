//! Claim extraction heuristics.

pub mod cmd;
pub mod faithfulness;
pub mod heuristics;
pub mod hints;
pub mod normalize;
pub mod word_match;

pub use cmd::extract_via_cmd;
pub use faithfulness::{assess_faithfulness, Faithfulness, DEFAULT_GROUNDING_THRESHOLD_PPM};
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

/// Convert a byte offset into the journaled `u32` domain.
///
/// Offsets past `u32::MAX` **saturate** (deliberately — not a silent wrap): a
/// markdown source at or beyond 4 GiB is far outside any realistic corpus, and
/// a saturated offset merely degrades "jump to source" precision for that
/// pathological file instead of failing the whole ingest. This mirrors the
/// existing line-number convention (`u32::try_from(..).unwrap_or(u32::MAX)`).
#[must_use]
pub fn byte_offset_u32(offset: usize) -> u32 {
    u32::try_from(offset).unwrap_or(u32::MAX)
}

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
                // On the heuristic path the claim's SOURCE SPAN is the line
                // itself, so the journaled byte range is the line's range in
                // the raw body — the same span-level semantics the LLM path
                // uses for its `CandidateSpan` (claims are paraphrases there;
                // claim-level offsets are ill-defined by design).
                char_start: byte_offset_u32(line.char_start),
                char_end: byte_offset_u32(line.char_start.saturating_add(line.text.len())),
                text: line.text.clone(),
                normalized_text: normalized,
                subject_hint: hints.subject_hint,
                predicate_hint: hints.predicate_hint,
                object_hint: hints.object_hint,
                confidence_ppm: hints.confidence_ppm,
                extractor_kind: extractor_kind.to_string(),
                // The heuristic extractor has no model and no prompt; empty is
                // the "not applicable" value (identical to the serde default
                // that pre-v1.1 events decode to).
                extractor_model: String::new(),
                prompt_version: String::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_doc(source: &str) -> (MarkdownDocument, SourceId) {
        let doc = MarkdownDocument::from_bytes("t.md", source.as_bytes()).expect("doc");
        let source_id = SourceId::try_from(doc.source_id.as_str()).expect("source id");
        (doc, source_id)
    }

    #[test]
    fn heuristic_claims_carry_line_span_byte_offsets() {
        // The heuristic path attributes each claim the byte range of its source
        // span (the line), so char_start..char_end must slice the RAW source
        // back to the claim text — including after a code fence, which shifts
        // byte offsets past the dropped lines.
        let source = "# Title\n\n```\ncode\n```\n\nDeploys happen on Friday.\n";
        let (doc, source_id) = parse_doc(source);
        let claims = extract_claims(&doc, &source_id, "demo", 0).expect("extract");
        assert!(!claims.is_empty(), "the deploy line must yield a claim");
        for claim in &claims {
            let start = usize::try_from(claim.payload.char_start).expect("start fits");
            let end = usize::try_from(claim.payload.char_end).expect("end fits");
            assert!(start <= end, "offsets must be ordered");
            assert!(end <= source.len(), "offsets must stay within the source");
            assert_eq!(
                &source[start..end],
                claim.payload.text,
                "span offsets must slice back to the claim's source line"
            );
        }
    }

    #[test]
    fn heuristic_claims_have_empty_model_provenance() {
        // No model and no prompt participate in heuristic extraction; the
        // provenance fields stay at their "not applicable" empty defaults while
        // extractor_kind still records the extractor.
        let (doc, source_id) = parse_doc("Deploys happen on Friday.\n");
        let claims = extract_claims(&doc, &source_id, "demo", 0).expect("extract");
        assert!(!claims.is_empty());
        for claim in &claims {
            assert_eq!(claim.payload.extractor_kind, EXTRACTOR_KIND_HEURISTIC_V1);
            assert_eq!(claim.payload.extractor_model, "");
            assert_eq!(claim.payload.prompt_version, "");
        }
    }

    #[test]
    fn byte_offset_u32_saturates_instead_of_wrapping() {
        assert_eq!(byte_offset_u32(0), 0);
        assert_eq!(byte_offset_u32(41), 41);
        let exact_max = usize::try_from(u32::MAX).expect("u32::MAX fits usize");
        assert_eq!(byte_offset_u32(exact_max), u32::MAX);
        assert_eq!(byte_offset_u32(usize::MAX), u32::MAX);
    }
}
