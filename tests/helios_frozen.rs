//! Network-free rendering-format smoke test for the checked-in Helios trophy.
//!
//! Semantic correctness belongs to the key-gated live Helios pipeline. This
//! test deliberately makes no oracle claims about a frozen artifact.

use std::path::PathBuf;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn repo_path(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

#[test]
fn helios_trophy_keeps_the_public_rendering_contract() -> TestResult {
    let trophy = std::fs::read_to_string(repo_path("examples/helios/onboarding.generated.md"))?;
    for heading in ["## Current", "## Stale", "## Conflicts"] {
        assert!(
            trophy.contains(heading),
            "missing rendering section {heading}"
        );
    }
    assert!(
        trophy.lines().any(|line| line.contains("source:")),
        "rendered claims must expose source provenance"
    );
    assert!(
        trophy.lines().any(|line| line.starts_with("- **claim_")),
        "rendered output must contain at least one formatted claim"
    );
    Ok(())
}
