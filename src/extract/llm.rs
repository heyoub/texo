//! LLM-backed extraction orchestration.
//!
//! This glues deterministic segmentation and faithfulness checks around the
//! nondeterministic proposer: segment markdown prose, ask the proposer for
//! atomic claims, and emit only grounded claims as NDJSON-ready records.

use serde::Serialize;

use crate::extract::faithfulness::assess_faithfulness;
use crate::extract::markdown::segment_candidates;
use crate::extract::normalize::normalize_line;
use crate::semantics::{Proposer, SemanticsError};

const UNKNOWN_HINT: &str = "unknown";

/// Failure while running extraction over a document.
#[derive(Debug, thiserror::Error)]
pub enum ExtractRunError {
    /// The Stage-1 proposer backend failed.
    #[error("stage-1 proposer failed")]
    Proposer(#[from] SemanticsError),
}

/// One emitted claim line matching the extractor-command NDJSON contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputClaim {
    /// Raw 1-based source line the claim span starts on.
    pub line_start: u32,
    /// Byte offset (inclusive start) of the claim source span.
    pub char_start: u32,
    /// Byte offset (exclusive end) of the claim source span.
    pub char_end: u32,
    /// Faithful claim text.
    pub text: String,
    /// Normalized claim text.
    pub normalized_text: String,
    /// Subject hint, or `unknown` when the proposer gave none.
    pub subject_hint: String,
    /// Predicate hint, or `unknown` when the proposer gave none.
    pub predicate_hint: String,
    /// Object hint; falls back to the normalized text when missing.
    pub object_hint: String,
    /// Confidence in parts-per-million.
    pub confidence_ppm: u32,
    /// Model identity that proposed this claim.
    pub extractor_model: String,
    /// Extraction prompt version.
    pub prompt_version: String,
}

fn provenance_from_fingerprint(fingerprint: &str) -> (String, String) {
    match fingerprint.rsplit_once('|') {
        Some((model, prompt)) => (model.to_string(), prompt.to_string()),
        None => (fingerprint.to_string(), String::new()),
    }
}

fn hint_or_unknown(value: String) -> String {
    if value.trim().is_empty() {
        UNKNOWN_HINT.to_string()
    } else {
        value
    }
}

fn byte_offset_u32(offset: usize) -> u32 {
    u32::try_from(offset).unwrap_or(u32::MAX)
}

/// Run the segment -> propose -> faithfulness pipeline over `source`.
///
/// # Errors
///
/// Returns [`ExtractRunError::Proposer`] when the proposer backend fails.
pub fn run_extraction(
    source: &str,
    proposer: &dyn Proposer,
    threshold_ppm: u32,
) -> Result<Vec<OutputClaim>, ExtractRunError> {
    // Cache identity and journaled provenance are split from the same proposer
    // fingerprint, so the cached output contract and recorded provenance cannot
    // drift from each other.
    let (extractor_model, prompt_version) = provenance_from_fingerprint(&proposer.fingerprint());
    let spans = segment_candidates(source);
    let mut out = Vec::new();
    for span in &spans {
        let proposals = proposer.propose(&span.text, &span.heading_path)?;
        for proposal in proposals {
            if !assess_faithfulness(&proposal.text, &span.text, threshold_ppm).grounded {
                continue;
            }
            let normalized_text = normalize_line(&proposal.text);
            let object_hint = if proposal.object.trim().is_empty() {
                normalized_text.clone()
            } else {
                proposal.object
            };
            out.push(OutputClaim {
                line_start: span.line_start,
                char_start: byte_offset_u32(span.char_start),
                char_end: byte_offset_u32(span.char_end),
                text: proposal.text,
                normalized_text,
                subject_hint: hint_or_unknown(proposal.subject),
                predicate_hint: hint_or_unknown(proposal.predicate),
                object_hint,
                confidence_ppm: proposal.confidence_ppm,
                extractor_model: extractor_model.clone(),
                prompt_version: prompt_version.clone(),
            });
        }
    }
    Ok(out)
}

/// Write `claims` as newline-delimited JSON.
///
/// # Errors
///
/// Returns I/O errors from writing or JSON serialization.
pub fn write_ndjson(claims: &[OutputClaim], out: &mut impl std::io::Write) -> std::io::Result<()> {
    for claim in claims {
        serde_json::to_writer(&mut *out, claim).map_err(std::io::Error::other)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::extract::faithfulness::DEFAULT_GROUNDING_THRESHOLD_PPM;
    use crate::semantics::ProposedClaim;

    use super::*;

    struct ScriptedProposer {
        proposals: Vec<ProposedClaim>,
        fail: bool,
    }

    impl ScriptedProposer {
        fn ok(proposals: Vec<ProposedClaim>) -> Self {
            Self {
                proposals,
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                proposals: Vec::new(),
                fail: true,
            }
        }
    }

    impl Proposer for ScriptedProposer {
        fn propose(
            &self,
            _span_text: &str,
            _heading_path: &[String],
        ) -> Result<Vec<ProposedClaim>, SemanticsError> {
            if self.fail {
                return Err(SemanticsError::ResultCountMismatch {
                    expected: 1,
                    actual: 0,
                });
            }
            Ok(self.proposals.clone())
        }

        fn fingerprint(&self) -> String {
            "scripted".to_string()
        }
    }

    fn proposal(text: &str, subject: &str, object: &str) -> ProposedClaim {
        ProposedClaim {
            text: text.to_string(),
            subject: subject.to_string(),
            predicate: "is".to_string(),
            object: object.to_string(),
            confidence_ppm: 800_000,
        }
    }

    const DOC: &str = "# Title\n\nDeploys moved to Tuesday.\n";

    #[test]
    fn grounded_proposal_is_emitted_with_span_line_and_hints() {
        let proposer = ScriptedProposer::ok(vec![proposal(
            "Deploys moved to Tuesday.",
            "deploys",
            "Tuesday",
        )]);
        let out =
            run_extraction(DOC, &proposer, DEFAULT_GROUNDING_THRESHOLD_PPM).expect("run succeeds");
        assert_eq!(out.len(), 1);
        let claim = &out[0];
        assert_eq!(claim.line_start, 3);
        assert_eq!(claim.text, "Deploys moved to Tuesday.");
        assert_eq!(claim.subject_hint, "deploys");
        assert_eq!(claim.object_hint, "Tuesday");
        assert_eq!(claim.confidence_ppm, 800_000);
        assert!(!claim.normalized_text.is_empty());
    }

    #[test]
    fn emitted_offsets_are_the_spans_byte_range() {
        let proposer = ScriptedProposer::ok(vec![proposal(
            "Deploys moved to Tuesday.",
            "deploys",
            "Tuesday",
        )]);
        let out =
            run_extraction(DOC, &proposer, DEFAULT_GROUNDING_THRESHOLD_PPM).expect("run succeeds");
        let claim = &out[0];
        let start = usize::try_from(claim.char_start).expect("start fits");
        let end = usize::try_from(claim.char_end).expect("end fits");
        assert!(start < end);
        assert!(end <= DOC.len());
        assert_eq!(&DOC[start..end], "Deploys moved to Tuesday.");
    }

    #[test]
    fn provenance_is_split_from_the_proposer_fingerprint() {
        let proposer = ScriptedProposer::ok(vec![proposal(
            "Deploys moved to Tuesday.",
            "deploys",
            "Tuesday",
        )]);
        let out =
            run_extraction(DOC, &proposer, DEFAULT_GROUNDING_THRESHOLD_PPM).expect("run succeeds");
        assert_eq!(out[0].extractor_model, "scripted");
        assert_eq!(out[0].prompt_version, "");
    }

    #[test]
    fn provenance_from_fingerprint_splits_model_and_prompt() {
        assert_eq!(
            provenance_from_fingerprint("openrouter:anthropic/claude-opus-4.8|propose-v3"),
            (
                "openrouter:anthropic/claude-opus-4.8".to_string(),
                "propose-v3".to_string()
            )
        );
        assert_eq!(
            provenance_from_fingerprint("scripted"),
            ("scripted".to_string(), String::new())
        );
    }

    #[test]
    fn ungrounded_proposal_is_dropped_by_the_gate() {
        let proposer = ScriptedProposer::ok(vec![proposal(
            "Kubernetes autoscaler tuned aggressively",
            "k8s",
            "autoscaler",
        )]);
        let out =
            run_extraction(DOC, &proposer, DEFAULT_GROUNDING_THRESHOLD_PPM).expect("run succeeds");
        assert!(out.is_empty());
    }

    #[test]
    fn blank_hints_become_unknown_and_object_defaults_to_normalized() {
        let proposer = ScriptedProposer::ok(vec![proposal("Deploys moved to Tuesday.", "  ", "")]);
        let out =
            run_extraction(DOC, &proposer, DEFAULT_GROUNDING_THRESHOLD_PPM).expect("run succeeds");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].subject_hint, "unknown");
        assert_eq!(out[0].predicate_hint, "is");
        assert_eq!(out[0].object_hint, out[0].normalized_text);
    }

    #[test]
    fn proposer_failure_propagates() {
        let error =
            run_extraction(DOC, &ScriptedProposer::failing(), 600_000).expect_err("propagates");
        assert!(matches!(error, ExtractRunError::Proposer(_)));
    }

    #[test]
    fn document_with_no_prose_spans_yields_nothing() {
        let doc = "# Heading\n\n```\nlet x = 1;\n```\n";
        let proposer = ScriptedProposer::ok(vec![proposal("anything", "a", "b")]);
        let out = run_extraction(doc, &proposer, 600_000).expect("run succeeds");
        assert!(out.is_empty());
    }

    #[test]
    fn write_ndjson_emits_one_json_object_per_line() {
        let claims = vec![
            OutputClaim {
                line_start: 3,
                char_start: 9,
                char_end: 11,
                text: "A.".to_string(),
                normalized_text: "a".to_string(),
                subject_hint: "a".to_string(),
                predicate_hint: "is".to_string(),
                object_hint: "x".to_string(),
                confidence_ppm: 700_000,
                extractor_model: "openrouter:test-model".to_string(),
                prompt_version: "propose-v3".to_string(),
            },
            OutputClaim {
                line_start: 5,
                char_start: 13,
                char_end: 15,
                text: "B.".to_string(),
                normalized_text: "b".to_string(),
                subject_hint: "b".to_string(),
                predicate_hint: "is".to_string(),
                object_hint: "y".to_string(),
                confidence_ppm: 600_000,
                extractor_model: "openrouter:test-model".to_string(),
                prompt_version: "propose-v3".to_string(),
            },
        ];
        let mut buf = Vec::new();
        write_ndjson(&claims, &mut buf).expect("write succeeds");
        let text = String::from_utf8(buf).expect("utf8 output");
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).expect("json line");
        assert_eq!(first["line_start"], 3);
        assert_eq!(first["text"], "A.");
        assert_eq!(first["confidence_ppm"], 700_000);
        assert_eq!(first["char_start"], 9);
        assert_eq!(first["char_end"], 11);
        assert_eq!(first["extractor_model"], "openrouter:test-model");
        assert_eq!(first["prompt_version"], "propose-v3");
    }
}
