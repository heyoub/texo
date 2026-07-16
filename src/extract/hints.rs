//! Subject, predicate, and object hints.

use super::normalize::normalize_line;
use super::word_match::{contains_phrase, contains_word};

/// Parsed hint triple from a claim line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimHints {
    /// Subject hint slug.
    pub subject_hint: String,
    /// Predicate hint.
    pub predicate_hint: String,
    /// Object hint.
    pub object_hint: String,
    /// Confidence in parts per million.
    pub confidence_ppm: u32,
}

const KEYWORDS: &[(&str, u32)] = &[
    ("decided", 900_000),
    ("decision", 900_000),
    ("changed", 900_000),
    ("owner", 800_000),
    ("owns", 800_000),
    ("process", 800_000),
    ("approval", 800_000),
    ("is", 650_000),
    ("are", 650_000),
    ("uses", 650_000),
    ("use", 650_000),
    ("must", 650_000),
    ("should", 650_000),
    ("moved", 650_000),
    ("replaced", 650_000),
    ("no longer", 650_000),
    ("deprecated", 650_000),
    ("deploy", 650_000),
    ("release", 650_000),
    ("friday", 700_000),
    ("tuesday", 700_000),
    ("monday", 700_000),
    ("wednesday", 700_000),
    ("thursday", 700_000),
];

/// Derive hints from a raw markdown line.
#[must_use]
pub fn hints_from_line(line: &str) -> Option<ClaimHints> {
    hints_from_line_normalized(line, &normalize_line(line))
}

/// Like [`hints_from_line`] for callers that already normalized the line —
/// the extract hot loop otherwise normalizes every candidate line twice.
#[must_use]
pub fn hints_from_line_normalized(line: &str, normalized: &str) -> Option<ClaimHints> {
    if !super::heuristics::is_claim_line(line) {
        return None;
    }
    if normalized.is_empty() {
        return None;
    }

    let lower = line.to_ascii_lowercase();
    let (confidence_ppm, _) = detect_confidence(&lower);

    let predicate_hint = detect_predicate(&lower);
    let subject_hint = detect_subject(&lower, normalized);
    let object_hint = detect_object(normalized, predicate_hint);

    Some(ClaimHints {
        subject_hint,
        predicate_hint: predicate_hint.to_string(),
        object_hint,
        confidence_ppm,
    })
}

fn detect_confidence(lower: &str) -> (u32, bool) {
    for (needle, ppm) in KEYWORDS {
        if contains_phrase(lower, needle) {
            return (*ppm, true);
        }
    }
    (super::DEFAULT_CONFIDENCE_PPM, false)
}

fn detect_predicate(lower: &str) -> &'static str {
    if contains_word(lower, "uses") || contains_word(lower, "use") {
        "uses"
    } else if contains_word(lower, "is") {
        "is"
    } else if contains_word(lower, "must") {
        "must"
    } else if contains_word(lower, "should") {
        "should"
    } else if contains_word(lower, "owns") || contains_word(lower, "owner") {
        "owns"
    } else if contains_word(lower, "changed")
        || contains_word(lower, "moved")
        || contains_word(lower, "replaced")
    {
        "changed"
    } else {
        "unknown"
    }
}

fn detect_subject(lower: &str, normalized: &str) -> String {
    if contains_word(lower, "deploy") || contains_word(lower, "deploys") {
        return "deploy-process".to_string();
    }
    if contains_word(lower, "release") || contains_word(lower, "releases") {
        return "release-process".to_string();
    }
    if contains_word(lower, "owner") || contains_word(lower, "owns") {
        return "ownership".to_string();
    }
    if contains_word(lower, "approval") {
        return "approval-process".to_string();
    }
    slugify_words(normalized, 3, 6)
}

fn detect_object(normalized: &str, predicate: &str) -> String {
    for day in ["monday", "tuesday", "wednesday", "thursday", "friday"] {
        if contains_word(normalized, day) {
            return day.to_string();
        }
    }
    if predicate == "unknown" {
        return normalized.to_string();
    }
    if let Some(idx) = normalized.find(predicate) {
        let rest = normalized[idx + predicate.len()..].trim();
        if rest.is_empty() {
            normalized.to_string()
        } else {
            rest.to_string()
        }
    } else {
        normalized.to_string()
    }
}

fn slugify_words(text: &str, min: usize, max: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().take(max).collect();
    let count = words.len().clamp(min, max);
    words
        .into_iter()
        .take(count)
        .map(|w| {
            w.chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friday_deploy_subject() {
        let hints = hints_from_line("Deploys happen on Friday.").expect("claim");
        assert_eq!(hints.subject_hint, "deploy-process");
    }

    #[test]
    fn meeting_notes_deploy_subject() {
        let hints = hints_from_line("Decision: deploys moved to Tuesday.").expect("claim");
        assert_eq!(hints.subject_hint, "deploy-process");
        assert!(hints.confidence_ppm >= 650_000);
    }

    #[test]
    fn predicate_owns_not_matched_inside_downstream() {
        // "owns" must not match inside "downstream" (d-owns-tream); no whole-word
        // predicate present, so the predicate is "unknown".
        assert_eq!(
            detect_predicate("downstream batch jobs run nightly"),
            "unknown"
        );
    }

    #[test]
    fn predicate_owns_matched_as_whole_word() {
        assert_eq!(detect_predicate("alice owns the release"), "owns");
    }

    #[test]
    fn subject_ownership_not_matched_inside_downstream() {
        // "owns" must not match inside "downstream" when deriving the subject.
        let subject = detect_subject("downstream batch jobs run nightly", "downstream batch jobs");
        assert_ne!(subject, "ownership");
    }

    #[test]
    fn non_claim_line_yields_no_hints() {
        // A non-claim line (heuristics reject it) must produce None, not an
        // empty-hint triple.
        assert!(hints_from_line("").is_none());
        assert!(hints_from_line("# Heading").is_none());
    }

    #[test]
    fn predicate_must_and_should() {
        assert_eq!(detect_predicate("deploys must happen on friday"), "must");
        assert_eq!(detect_predicate("releases should be reviewed"), "should");
    }

    #[test]
    fn subject_ownership_and_approval() {
        // "owner"/"owns" map to ownership; "approval" maps to approval-process.
        assert_eq!(
            detect_subject("the owner of billing is alice", "the owner of billing"),
            "ownership"
        );
        assert_eq!(
            detect_subject("approval requires two reviewers", "approval requires two"),
            "approval-process"
        );
    }

    #[test]
    fn object_after_predicate_when_no_day_present() {
        // No weekday in the text, a known predicate present: the object is the
        // text following the predicate token, trimmed.
        let obj = detect_object("billing owns the payments domain", "owns");
        assert_eq!(obj, "the payments domain");
    }

    #[test]
    fn object_falls_back_to_full_text_when_predicate_at_end() {
        // Predicate is the trailing token, so the remainder is empty and the
        // object falls back to the full normalized text.
        let obj = detect_object("the team owns", "owns");
        assert_eq!(obj, "the team owns");
    }

    #[test]
    fn object_full_text_when_predicate_absent_from_text() {
        // Predicate token is not a substring of the normalized text (e.g. derived
        // differently): the object is the whole normalized text.
        let obj = detect_object("alpha beta gamma", "uses");
        assert_eq!(obj, "alpha beta gamma");
    }

    #[test]
    fn confidence_decided_not_matched_inside_undecided() {
        // "decided" must not match inside "undecided" (no other keyword present);
        // confidence falls back to the default.
        let (ppm, hit) = detect_confidence("scheduling remains undecided");
        assert!(!hit, "must not detect a keyword inside 'undecided'");
        assert_eq!(ppm, super::super::DEFAULT_CONFIDENCE_PPM);
    }
}
