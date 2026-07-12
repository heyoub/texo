//! Shared heuristic lexicons.
//!
//! The claim-extraction and relation heuristics decide when a newer claim
//! *replaces* an older one by scanning for replacement wording. That wordlist
//! was previously duplicated in the ingest path and the relate path and had
//! drifted apart in both content and matching semantics (one used whole-word
//! matching, the other raw substring, so `"now"` matched inside `"known"`).
//! This module is the single source of truth, matched only through
//! [`contains_replacement_signal`] so every caller agrees by construction.

use crate::extract::word_match::contains_phrase;

/// Wording that marks a claim as replacing an earlier one on the same subject.
///
/// Matched as whole words/phrases, never as substrings: `"now"` must not fire
/// inside `"known"` / `"snow"` / `"knowledge"`, which would fabricate a
/// supersession from an unrelated claim.
pub const REPLACEMENT_SIGNALS: &[&str] = &[
    "moved",
    "changed",
    "now",
    "no longer",
    "replaced",
    "instead",
    "new process",
    "as of",
    "decided",
];

/// Returns true when `text` carries replacement wording as a whole word/phrase.
#[must_use]
pub fn contains_replacement_signal(text: &str) -> bool {
    REPLACEMENT_SIGNALS
        .iter()
        .any(|signal| contains_phrase(text, signal))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_whole_words_only() {
        assert!(contains_replacement_signal(
            "the target now ships on tuesday"
        ));
        assert!(contains_replacement_signal("deploys moved to friday"));
        assert!(contains_replacement_signal("as of q3 the owner is bob"));
    }

    #[test]
    fn does_not_fire_on_substrings_of_now() {
        // The exact false-positive the split wordlists produced.
        assert!(!contains_replacement_signal("this is a known issue"));
        assert!(!contains_replacement_signal(
            "the service runs nowhere near capacity"
        ));
        assert!(!contains_replacement_signal(
            "snowfall metrics are recorded nightly"
        ));
        assert!(!contains_replacement_signal(
            "knowledge base articles are indexed"
        ));
    }
}
