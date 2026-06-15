//! Subject, predicate, and object hints.

use super::normalize::normalize_line;

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
    ("decision:", 900_000),
    ("changed", 900_000),
    ("owner", 800_000),
    ("owns", 800_000),
    ("process", 800_000),
    ("approval", 800_000),
    (" is ", 650_000),
    (" are ", 650_000),
    (" uses ", 650_000),
    (" use ", 650_000),
    (" must ", 650_000),
    (" should ", 650_000),
    (" moved", 650_000),
    (" replaced", 650_000),
    (" no longer", 650_000),
    (" deprecated", 650_000),
    (" deploy", 650_000),
    (" release", 650_000),
    (" friday", 700_000),
    (" tuesday", 700_000),
    (" monday", 700_000),
    (" wednesday", 700_000),
    (" thursday", 700_000),
];

/// Derive hints from a raw markdown line.
pub fn hints_from_line(line: &str) -> Option<ClaimHints> {
    let normalized = normalize_line(line);
    if normalized.is_empty() || !super::heuristics::is_claim_line(line) {
        return None;
    }

    let lower = line.to_ascii_lowercase();
    let (confidence_ppm, _) = detect_confidence(&lower);

    let predicate_hint = detect_predicate(&lower);
    let subject_hint = detect_subject(&lower, &normalized);
    let object_hint = detect_object(&normalized, predicate_hint);

    Some(ClaimHints {
        subject_hint,
        predicate_hint: predicate_hint.to_string(),
        object_hint,
        confidence_ppm,
    })
}

fn detect_confidence(lower: &str) -> (u32, bool) {
    for (needle, ppm) in KEYWORDS {
        if lower.contains(needle) {
            return (*ppm, true);
        }
    }
    (500_000, false)
}

fn detect_predicate(lower: &str) -> &'static str {
    if lower.contains("uses") || lower.contains("use ") {
        "uses"
    } else if lower.contains(" is ") || lower.starts_with("is ") {
        "is"
    } else if lower.contains(" must ") {
        "must"
    } else if lower.contains(" should ") {
        "should"
    } else if lower.contains("owns") || lower.contains("owner") {
        "owns"
    } else if lower.contains("changed") || lower.contains("moved") || lower.contains("replaced") {
        "changed"
    } else {
        "unknown"
    }
}

fn detect_subject(lower: &str, normalized: &str) -> String {
    if lower.contains("deploy") {
        return "deploy-process".to_string();
    }
    if lower.contains("release") {
        return "release-process".to_string();
    }
    if lower.contains("owner") || lower.contains("owns") {
        return "ownership".to_string();
    }
    if lower.contains("approval") {
        return "approval-process".to_string();
    }
    slugify_words(normalized, 3, 6)
}

fn detect_object(normalized: &str, predicate: &str) -> String {
    for day in ["monday", "tuesday", "wednesday", "thursday", "friday"] {
        if normalized.contains(day) {
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
}
