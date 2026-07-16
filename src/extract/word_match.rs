//! Whole-word and whole-phrase matching helpers.
//!
//! Keyword scans over natural-language text must match on word boundaries, not
//! raw substrings: otherwise "is" matches inside "this"/"basis", "owns" inside
//! "downstream", "now" inside "known", "decided" inside "undecided", and so on.
//! These helpers split the candidate into ASCII-alphanumeric tokens and compare
//! whole tokens (a phrase compares a run of consecutive tokens).

/// Tokenize `text` into borrowed ASCII-alphanumeric words. Matching uses
/// `eq_ignore_ascii_case`, so scanning never allocates copies of the source.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
}

/// Returns true when `needle` occurs as a whole word in `text`.
///
/// `needle` is matched case-insensitively against complete word tokens, so it
/// will not match when it is only a substring of a larger word.
#[must_use]
pub fn contains_word(text: &str, needle: &str) -> bool {
    let needle = needle.trim();
    if needle.is_empty() {
        return false;
    }
    words(text).any(|word| word.eq_ignore_ascii_case(needle))
}

/// Returns true when `phrase` (one or more whitespace-separated words) occurs as
/// a consecutive run of whole words in `text`.
#[must_use]
pub fn contains_phrase(text: &str, phrase: &str) -> bool {
    let needle: Vec<&str> = phrase.split_whitespace().collect();
    if needle.is_empty() {
        return false;
    }
    if needle.len() == 1 {
        return contains_word(text, needle[0]);
    }
    let haystack = words(text).collect::<Vec<_>>();
    if haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(&needle)
            .all(|(word, wanted)| word.eq_ignore_ascii_case(wanted))
    })
}

/// Returns true when any of `needles` occurs as a whole word or phrase in `text`.
#[must_use]
pub fn contains_any(text: &str, needles: &[&str]) -> bool {
    let haystack = words(text).collect::<Vec<_>>();
    needles.iter().any(|needle| {
        let wanted = needle.split_whitespace().collect::<Vec<_>>();
        !wanted.is_empty()
            && haystack.windows(wanted.len()).any(|window| {
                window
                    .iter()
                    .zip(&wanted)
                    .all(|(word, wanted)| word.eq_ignore_ascii_case(wanted))
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_word_not_substring() {
        assert!(!contains_word("this is the basis", "owns"));
        assert!(!contains_word("a known issue", "now"));
        assert!(!contains_word("undecided plan", "decided"));
        assert!(!contains_word("downstream jobs", "owns"));
    }

    #[test]
    fn whole_word_positive() {
        assert!(contains_word("alice owns it", "owns"));
        assert!(contains_word("it is done", "is"));
        assert!(contains_word("Friday deploy", "friday"));
    }

    #[test]
    fn phrase_matches_consecutive_words() {
        assert!(contains_phrase("this is no longer valid", "no longer"));
        assert!(!contains_phrase("no one longer waits", "no longer"));
        assert!(!contains_phrase("knowledge longer", "no longer"));
    }

    #[test]
    fn contains_any_scans_all() {
        assert!(contains_any("deploys moved today", &["changed", "moved"]));
        assert!(!contains_any("a known plan", &["now", "owns"]));
    }

    #[test]
    fn empty_needle_never_matches() {
        // An empty or whitespace-only needle must never match, even against
        // non-empty text: the empty-needle guards in contains_word and
        // contains_phrase short-circuit to false rather than vacuously matching.
        assert!(!contains_word("any text here", ""));
        assert!(!contains_word("any text here", "   "));
        assert!(!contains_phrase("any text here", ""));
        assert!(!contains_phrase("any text here", "   "));
        // contains_any over only-empty needles is likewise false.
        assert!(!contains_any("any text here", &["", "  "]));
    }
}
