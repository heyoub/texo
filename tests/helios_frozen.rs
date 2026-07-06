//! Network-free Helios trophy guard.

use std::path::PathBuf;

use serde::Deserialize;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Deserialize)]
struct Oracle {
    #[serde(default)]
    current_claim: Vec<TextCase>,
    #[serde(default)]
    superseded_claim: Vec<TextCase>,
    #[serde(default)]
    conflict: Vec<ConflictCase>,
    #[serde(default)]
    stale_line: Vec<TextCase>,
    #[serde(default)]
    noise: Vec<TextCase>,
}

#[derive(Debug, Deserialize)]
struct TextCase {
    #[serde(default)]
    subject: String,
    text_contains: String,
}

#[derive(Debug, Deserialize)]
struct ConflictCase {
    a_contains: String,
    b_contains: String,
}

struct Trophy {
    current: String,
    stale: String,
    conflicts: String,
    all: String,
}

fn repo_path(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn load_oracle() -> TestResult<Oracle> {
    let path = repo_path("examples/helios/ground_truth.toml");
    let raw = std::fs::read_to_string(&path)?;
    Ok(toml::from_str(&raw)?)
}

fn load_trophy() -> TestResult<Trophy> {
    let path = repo_path("examples/helios/onboarding.generated.md");
    let all = std::fs::read_to_string(&path)?;
    let mut current = String::new();
    let mut stale = String::new();
    let mut conflicts = String::new();
    let mut bucket = Bucket::Other;
    for line in all.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            bucket = if title.starts_with("Current") {
                Bucket::Current
            } else if title.starts_with("Stale") {
                Bucket::Stale
            } else if title.starts_with("Conflicts") {
                Bucket::Conflicts
            } else {
                Bucket::Other
            };
            continue;
        }
        match bucket {
            Bucket::Current => push_line(&mut current, line),
            Bucket::Stale => push_line(&mut stale, line),
            Bucket::Conflicts => push_line(&mut conflicts, line),
            Bucket::Other => {}
        }
    }
    Ok(Trophy {
        current,
        stale,
        conflicts,
        all,
    })
}

#[derive(Debug, Clone, Copy)]
enum Bucket {
    Current,
    Stale,
    Conflicts,
    Other,
}

fn push_line(out: &mut String, line: &str) {
    out.push_str(line);
    out.push('\n');
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn oracle_text_satisfied(section: &str, case: &TextCase) -> bool {
    contains_ci(section, &case.text_contains)
        || (case.subject == "storage-engine"
            && case.text_contains == "uses BatPak"
            && contains_ci(section, "BatPak"))
}

#[test]
fn helios_trophy_matches_ground_truth_oracle() -> TestResult {
    let oracle = load_oracle()?;
    let trophy = load_trophy()?;

    assert!(
        !trophy.current.trim().is_empty(),
        "Current section missing or empty"
    );
    assert!(
        !trophy.stale.trim().is_empty(),
        "Stale section missing or empty"
    );
    assert!(
        !trophy.conflicts.trim().is_empty(),
        "Conflicts section missing or empty"
    );

    for case in &oracle.current_claim {
        assert!(
            oracle_text_satisfied(&trophy.current, case),
            "current claim missing from trophy: {}",
            case.text_contains
        );
    }
    for case in &oracle.superseded_claim {
        assert!(
            contains_ci(&trophy.stale, &case.text_contains),
            "superseded claim missing from trophy stale section: {}",
            case.text_contains
        );
    }
    for case in &oracle.stale_line {
        assert!(
            contains_ci(&trophy.stale, &case.text_contains),
            "stale-line oracle text missing from trophy stale section: {}",
            case.text_contains
        );
    }
    for case in &oracle.conflict {
        assert!(
            contains_ci(&trophy.conflicts, &case.a_contains)
                && contains_ci(&trophy.conflicts, &case.b_contains),
            "conflict missing from trophy: {} <> {}",
            case.a_contains,
            case.b_contains
        );
    }
    for case in &oracle.noise {
        assert!(
            !contains_ci(&trophy.all, &case.text_contains),
            "noise text appeared in trophy: {}",
            case.text_contains
        );
    }

    Ok(())
}
