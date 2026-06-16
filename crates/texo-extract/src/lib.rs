//! texo Stage-1 extraction orchestration.
//!
//! Glues the deterministic stages around the one nondeterministic step (the LLM
//! proposer): segment a document into prose spans ([`segment_candidates`], Stage
//! 0), ask an injected [`Proposer`] for atomic claims per span (Stage 1), and keep
//! only claims grounded in their span by the deterministic faithfulness gate
//! ([`assess_faithfulness`], Stage 2). The proposer is a trait so this whole
//! orchestration is unit-tested with a scripted stub — no model, no network.
//!
//! The surviving claims are emitted as newline-delimited JSON matching the
//! `extract_via_cmd` contract in `texo-core`, so this binary drops in behind the
//! existing external-extractor seam and `texo-core` stays HTTP/LLM-free.

use serde::Serialize;
use texo_core::{assess_faithfulness, normalize_line, segment_candidates, Proposer, SemanticsError};

/// Hint value used when the proposer leaves a subject/predicate field blank,
/// matching the `extract_via_cmd` adapter's own default.
const UNKNOWN_HINT: &str = "unknown";

/// Failure while running extraction over a document.
#[derive(Debug, thiserror::Error)]
pub enum ExtractRunError {
    /// The Stage-1 proposer backend failed.
    #[error("stage-1 proposer failed")]
    Proposer(#[from] SemanticsError),
}

/// One emitted claim line, matching the `extract_via_cmd` NDJSON contract
/// (texo-core's `CmdClaimLine`). Field names are the wire contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputClaim {
    /// Raw 1-based source line the claim's span starts on.
    pub line_start: u32,
    /// Faithful claim text.
    pub text: String,
    /// Normalized claim text (texo-core's `normalize_line`).
    pub normalized_text: String,
    /// Subject hint, or [`UNKNOWN_HINT`] when the proposer gave none.
    pub subject_hint: String,
    /// Predicate hint, or [`UNKNOWN_HINT`] when the proposer gave none.
    pub predicate_hint: String,
    /// Object hint; falls back to the normalized text when the proposer gave none.
    pub object_hint: String,
    /// Confidence in parts-per-million.
    pub confidence_ppm: u32,
}

/// Non-blank value, else [`UNKNOWN_HINT`].
fn hint_or_unknown(value: String) -> String {
    if value.trim().is_empty() {
        UNKNOWN_HINT.to_owned()
    } else {
        value
    }
}

/// Run the Stage 0→1→2 pipeline over `source`.
///
/// Segments the document, proposes atomic claims per prose span, and retains only
/// those grounded in their span at `threshold_ppm`. Returns the emit-ready claims
/// in document order; a proposer failure aborts (the ingest must not journal a
/// partial, silently-truncated extraction).
pub fn run_extraction(
    source: &str,
    proposer: &dyn Proposer,
    threshold_ppm: u32,
) -> Result<Vec<OutputClaim>, ExtractRunError> {
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
                text: proposal.text,
                normalized_text,
                subject_hint: hint_or_unknown(proposal.subject),
                predicate_hint: hint_or_unknown(proposal.predicate),
                object_hint,
                confidence_ppm: proposal.confidence_ppm,
            });
        }
    }
    Ok(out)
}

/// Write `claims` as newline-delimited JSON to `out`.
///
/// A serialization failure (which a plain `OutputClaim` should never produce) is
/// surfaced as an I/O error rather than silently dropped.
pub fn write_ndjson(claims: &[OutputClaim], out: &mut impl std::io::Write) -> std::io::Result<()> {
    for claim in claims {
        serde_json::to_writer(&mut *out, claim).map_err(std::io::Error::other)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use texo_core::ProposedClaim;

    /// Proposer stub: returns a fixed set of proposals for every span, or fails.
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
    }

    fn proposal(text: &str, subject: &str, object: &str) -> ProposedClaim {
        ProposedClaim {
            text: text.to_owned(),
            subject: subject.to_owned(),
            predicate: "is".to_owned(),
            object: object.to_owned(),
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
        let out = run_extraction(DOC, &proposer, texo_core::DEFAULT_GROUNDING_THRESHOLD_PPM)
            .expect("run");
        assert_eq!(out.len(), 1);
        let c = &out[0];
        assert_eq!(c.line_start, 3, "the prose span starts on raw line 3");
        assert_eq!(c.text, "Deploys moved to Tuesday.");
        assert_eq!(c.subject_hint, "deploys");
        assert_eq!(c.object_hint, "Tuesday");
        assert_eq!(c.confidence_ppm, 800_000);
        assert!(!c.normalized_text.is_empty());
    }

    #[test]
    fn ungrounded_proposal_is_dropped_by_the_gate() {
        // The proposer hallucinates a claim with no tokens from the span.
        let proposer = ScriptedProposer::ok(vec![proposal(
            "Kubernetes autoscaler tuned aggressively",
            "k8s",
            "autoscaler",
        )]);
        let out = run_extraction(DOC, &proposer, texo_core::DEFAULT_GROUNDING_THRESHOLD_PPM)
            .expect("run");
        assert!(out.is_empty(), "hallucinated claim must not survive the gate");
    }

    #[test]
    fn blank_hints_become_unknown_and_object_defaults_to_normalized() {
        let proposer = ScriptedProposer::ok(vec![proposal("Deploys moved to Tuesday.", "  ", "")]);
        let out = run_extraction(DOC, &proposer, texo_core::DEFAULT_GROUNDING_THRESHOLD_PPM)
            .expect("run");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].subject_hint, "unknown");
        assert_eq!(out[0].predicate_hint, "is");
        assert_eq!(out[0].object_hint, out[0].normalized_text);
    }

    #[test]
    fn proposer_failure_propagates() {
        let err = run_extraction(DOC, &ScriptedProposer::failing(), 600_000)
            .expect_err("must propagate");
        assert!(matches!(err, ExtractRunError::Proposer(_)));
    }

    #[test]
    fn document_with_no_prose_spans_yields_nothing() {
        // Only a heading and a fenced code block — no prose to extract.
        let doc = "# Heading\n\n```\nlet x = 1;\n```\n";
        let proposer = ScriptedProposer::ok(vec![proposal("anything", "a", "b")]);
        let out = run_extraction(doc, &proposer, 600_000).expect("run");
        assert!(out.is_empty(), "no prose spans -> no claims");
    }

    #[test]
    fn write_ndjson_emits_one_json_object_per_line() {
        let claims = vec![
            OutputClaim {
                line_start: 3,
                text: "A.".to_owned(),
                normalized_text: "a".to_owned(),
                subject_hint: "a".to_owned(),
                predicate_hint: "is".to_owned(),
                object_hint: "x".to_owned(),
                confidence_ppm: 700_000,
            },
            OutputClaim {
                line_start: 5,
                text: "B.".to_owned(),
                normalized_text: "b".to_owned(),
                subject_hint: "b".to_owned(),
                predicate_hint: "is".to_owned(),
                object_hint: "y".to_owned(),
                confidence_ppm: 600_000,
            },
        ];
        let mut buf = Vec::new();
        write_ndjson(&claims, &mut buf).expect("write");
        let text = String::from_utf8(buf).expect("utf8");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).expect("json");
        assert_eq!(first["line_start"], 3);
        assert_eq!(first["text"], "A.");
        assert_eq!(first["confidence_ppm"], 700_000);
    }
}
