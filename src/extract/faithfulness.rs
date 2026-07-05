//! Stage 2 faithfulness gate.
//!
//! An LLM proposer (Stage 1) can hallucinate: assert a value or entity that its
//! source span never mentions. This gate is the deterministic guard against that.
//! It is intentionally **not** another model — it is a pure lexical-grounding
//! check that runs at replay-stable cost: a proposed claim is accepted only when a
//! sufficient fraction of its *content tokens* also appear in the source span.
//!
//! Grounding is measured as **token recall**: of the distinct content tokens in
//! the claim, how many occur in the source. A faithful paraphrase keeps the
//! entities, numbers, and key terms of its span and so scores high; an invented
//! fact introduces tokens (a different weekday, a wrong owner, a fabricated
//! number) that are absent from the span and so drives recall down. Recall is
//! reported in parts-per-million to stay integer (matching `confidence_ppm`) and
//! to keep the gate free of floating-point comparison.
//!
//! Content tokens are lowercased maximal alphanumeric runs of at least
//! [`MIN_TOKEN_LEN`] characters; everything shorter (punctuation, single letters)
//! carries no grounding signal and is dropped. No stopword list is used — a
//! function word that appears in the claim also appears in any span that supports
//! it, so it neither helps nor hurts, and a hardcoded stoplist would be exactly
//! the brittle wordlist this pipeline set out to retire.

use std::collections::HashSet;

/// Shortest alphanumeric run treated as a content token. One-character runs are
/// dropped as noise.
const MIN_TOKEN_LEN: usize = 2;

/// Default grounding-recall threshold, in parts-per-million. A claim is faithful
/// when at least this fraction of its distinct content tokens appear in the
/// source span. 0.60 tolerates light rephrasing while rejecting claims that
/// introduce unsupported entities or values.
pub const DEFAULT_GROUNDING_THRESHOLD_PPM: u32 = 600_000;

/// Outcome of the faithfulness gate for one claim against its source span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Faithfulness {
    /// Whether the claim cleared the grounding threshold.
    pub grounded: bool,
    /// Grounding recall in parts-per-million: distinct claim content tokens found
    /// in the source, over distinct claim content tokens total. `0` when the claim
    /// has no content tokens.
    pub recall_ppm: u32,
}

/// Lowercased distinct content tokens of `text` (maximal alphanumeric runs of at
/// least [`MIN_TOKEN_LEN`] chars).
fn content_tokens(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|tok| tok.chars().count() >= MIN_TOKEN_LEN)
        .map(str::to_lowercase)
        .collect()
}

/// Assess whether `claim_text` is grounded in `source_text` at `threshold_ppm`.
///
/// Returns the recall and the pass/fail verdict. A claim with no content tokens is
/// never grounded (recall `0`), so an empty or punctuation-only proposal cannot
/// slip through.
pub fn assess_faithfulness(
    claim_text: &str,
    source_text: &str,
    threshold_ppm: u32,
) -> Faithfulness {
    let claim = content_tokens(claim_text);
    if claim.is_empty() {
        return Faithfulness {
            grounded: false,
            recall_ppm: 0,
        };
    }
    let source = content_tokens(source_text);
    let present = claim.iter().filter(|tok| source.contains(*tok)).count();
    // Integer ppm: present/total scaled by 1e6. `claim` is non-empty here.
    let recall_ppm =
        u32::try_from(present as u64 * 1_000_000 / claim.len() as u64).unwrap_or(1_000_000);
    Faithfulness {
        grounded: recall_ppm >= threshold_ppm,
        recall_ppm,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u32 = DEFAULT_GROUNDING_THRESHOLD_PPM;

    #[test]
    fn exact_quote_is_fully_grounded() {
        let f = assess_faithfulness("Deploys moved to Tuesday.", "Deploys moved to Tuesday.", T);
        assert!(f.grounded);
        assert_eq!(f.recall_ppm, 1_000_000);
    }

    #[test]
    fn claim_subset_of_span_is_grounded() {
        // Every content token of the claim appears in the longer source span.
        let f = assess_faithfulness(
            "Bob owns release approval",
            "Bob owns release approval now, per the meeting.",
            T,
        );
        assert!(f.grounded);
        assert_eq!(f.recall_ppm, 1_000_000);
    }

    #[test]
    fn light_paraphrase_clears_threshold() {
        // "The platform stores data in Postgres" vs span: 4 of 5 content tokens
        // (platform, stores->absent, data->absent... pick tokens that mostly match)
        let f = assess_faithfulness(
            "The platform uses Postgres for storage",
            "The platform uses Postgres for durable storage of events",
            T,
        );
        assert!(f.grounded, "recall was {}", f.recall_ppm);
    }

    #[test]
    fn hallucinated_value_is_rejected() {
        // Source says Friday; the claim invents Saturday and noon — unsupported
        // tokens drag recall below 0.60.
        let f = assess_faithfulness(
            "Deploys happen on Saturday at noon",
            "Deploys happen on Friday.",
            T,
        );
        assert!(!f.grounded, "recall was {}", f.recall_ppm);
        assert!(f.recall_ppm < T);
    }

    #[test]
    fn fabricated_entity_is_rejected() {
        // None of the distinct content tokens of the claim are in the span.
        let f = assess_faithfulness(
            "Kubernetes autoscaler tuned aggressively",
            "Deploys happen on Friday.",
            T,
        );
        assert!(!f.grounded);
        assert_eq!(f.recall_ppm, 0);
    }

    #[test]
    fn empty_or_punctuation_claim_is_not_grounded() {
        assert!(!assess_faithfulness("", "anything here", T).grounded);
        assert_eq!(assess_faithfulness("", "anything here", T).recall_ppm, 0);
        assert!(!assess_faithfulness("—  .  !", "anything here", T).grounded);
    }

    #[test]
    fn single_char_tokens_do_not_count() {
        // "a" and "I" are below MIN_TOKEN_LEN and ignored on both sides, so only
        // "team" drives the verdict.
        let f = assess_faithfulness("a team", "I team", T);
        assert_eq!(
            f.recall_ppm, 1_000_000,
            "only 'team' counts and it is present"
        );
    }

    #[test]
    fn numbers_are_grounding_tokens() {
        // A version/number mismatch is a real hallucination the gate must catch.
        let grounded =
            assess_faithfulness("API v2 on port 9090", "The API v2 listens on port 9090", T);
        assert!(grounded.grounded);
        let wrong =
            assess_faithfulness("API v2 on port 1234", "The API v2 listens on port 9090", T);
        assert!(wrong.recall_ppm < grounded.recall_ppm);
    }

    #[test]
    fn threshold_is_inclusive_and_tunable() {
        // Two of three content tokens present -> 666666 ppm.
        let f = assess_faithfulness("alpha beta gamma", "alpha beta only", 0);
        assert_eq!(f.recall_ppm, 666_666);
        assert!(assess_faithfulness("alpha beta gamma", "alpha beta only", 666_666).grounded);
        assert!(!assess_faithfulness("alpha beta gamma", "alpha beta only", 666_667).grounded);
    }
}
