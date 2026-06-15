//! Claim line detection heuristics.

/// Returns true when a markdown line should become a claim in v0.
pub fn is_claim_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with('#') {
        let title = trimmed.trim_start_matches('#').trim();
        if title.len() < 4 {
            return false;
        }
    }
    if is_list_marker_only(trimmed) {
        return false;
    }

    let lower = format!(" {} ", line.to_ascii_lowercase());
    [
        " is ",
        " are ",
        " uses ",
        " use ",
        " must ",
        " should ",
        "owner",
        "owns",
        "process",
        "decided",
        "moved",
        "changed",
        "replaced",
        " no longer",
        " deprecated",
        " deploy",
        " release",
        " approval",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn is_list_marker_only(line: &str) -> bool {
    let stripped = line
        .trim_start_matches('-')
        .trim_start_matches('*')
        .trim_start_matches('+')
        .trim();
    stripped.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_blank_and_short_heading() {
        assert!(!is_claim_line(""));
        assert!(!is_claim_line("# OK"));
    }

    #[test]
    fn detects_deploy_line() {
        assert!(is_claim_line("Deploys happen on Friday."));
    }
}
