use super::write_relate;
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn rendered(value: &serde_json::Value) -> Result<(String, String), Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    write_relate(&mut out, &mut warnings, value)?;
    Ok((String::from_utf8(out)?, String::from_utf8(warnings)?))
}

#[test]
fn relate_complete_reports_outcome_and_page_budget() -> TestResult {
    let value = json!({
        "outcome": "complete",
        "claims_related": 3,
        "supersessions": [{}],
        "conflicts": [{}, {}],
        "candidate_pairs": 3,
        "candidate_pair_budget": 4096,
        "warnings": []
    });
    let (out, warnings) = rendered(&value)?;

    assert_eq!(
        out,
        "relate complete: 3 claims; supersessions: 1; conflicts: 2\n\
candidate pairs: 3 (page budget: 4096)\n"
    );
    assert!(warnings.is_empty());
    Ok(())
}

#[test]
fn relate_partial_reports_cursor_and_warnings() -> TestResult {
    let value = json!({
        "outcome": "partial",
        "claims_related": 5,
        "supersessions": [],
        "conflicts": [],
        "candidate_pairs": 2,
        "candidate_pair_budget": 2,
        "next_candidate_cursor": 4,
        "warnings": ["candidate page is incomplete"]
    });
    let (out, warnings) = rendered(&value)?;

    assert!(out.contains("relate partial"));
    assert!(out.contains("page budget: 2"));
    assert!(out.contains("resume candidate cursor: 4"));
    assert_eq!(warnings, "warning: candidate page is incomplete\n");
    Ok(())
}

#[test]
fn relate_rejudge_reports_transition_without_cache_provenance() -> TestResult {
    let value = json!({
        "outcome": "complete",
        "claims_related": 2,
        "supersessions": [],
        "conflicts": [],
        "candidate_pairs": 1,
        "candidate_pair_budget": 10,
        "rejudged_pair": {
            "older_claim": "claim_aaaaaaaaaaaa",
            "newer_claim": "claim_bbbbbbbbbbbb",
            "prior_relation": "supersedes",
            "fresh_relation": "conflict",
            "judge_fingerprint": "provider-secret-adjacent",
            "cache_key": "private-cache-key"
        }
    });
    let (out, warnings) = rendered(&value)?;

    assert!(out.contains(
        "rejudged pair claim_aaaaaaaaaaaa -> claim_bbbbbbbbbbbb: supersedes -> conflict"
    ));
    assert!(out.contains("first judgment remains authoritative"));
    assert!(!out.contains("provider-secret-adjacent"));
    assert!(!out.contains("private-cache-key"));
    assert!(warnings.is_empty());
    Ok(())
}
