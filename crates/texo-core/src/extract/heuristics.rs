//! Claim line detection heuristics.

/// Returns true when a markdown line should become a claim in v1.
pub fn is_claim_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    if trimmed.starts_with("##") {
        let title = trimmed.trim_start_matches('#').trim();
        return title.len() >= 4
            && [
                "process", "deploy", "release", "owner", "approval", "policy", "decision",
            ]
            .iter()
            .any(|needle| title.to_ascii_lowercase().contains(needle));
    }

    if trimmed.starts_with('#') && !trimmed.starts_with("##") {
        let title = trimmed.trim_start_matches('#').trim();
        if title.len() < 4 {
            return false;
        }
    }

    if is_list_marker_only(trimmed) {
        return false;
    }

    if trimmed.starts_with("Decision:") || trimmed.starts_with("decision:") {
        return true;
    }

    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        return has_claim_signal(
            trimmed
                .trim_start_matches('-')
                .trim_start_matches('*')
                .trim(),
        );
    }

    has_claim_signal(line)
}

fn has_claim_signal(line: &str) -> bool {
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
        "decision",
        "moved",
        "changed",
        "replaced",
        " no longer",
        " deprecated",
        " deploy",
        " release",
        " approval",
        " friday",
        " tuesday",
        " monday",
        " wednesday",
        " thursday",
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

    #[test]
    fn detects_decision_prefix() {
        assert!(is_claim_line("Decision: deploys moved to Tuesday."));
    }

    #[test]
    fn detects_bullet_claim() {
        assert!(is_claim_line("- Alice owns release approval."));
    }

    #[test]
    fn detects_policy_heading() {
        assert!(is_claim_line("## Deploy process"));
    }
}
