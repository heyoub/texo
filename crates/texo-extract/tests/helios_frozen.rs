//! FROZEN regression (no network): asserts the committed Helios trophy
//! (`examples/helios/onboarding.generated.md`) still encodes the 1/5 → 5/5
//! outcome. The live end-to-end proof in `helios_e2e.rs` is key-gated/ignored
//! (it burns model calls); this guards the *checked-in artifact* against drift on
//! every `cargo test`, so CI proves the headline claim without a key.
//!
//! Regenerate the trophy with `just demo-helios` (then re-snapshot) if the
//! pipeline legitimately changes the projection.

use std::path::PathBuf;

/// The three sections of the generated onboarding, split on `## ` headers.
struct Trophy {
    current: String,
    stale: String,
    conflicts: String,
}

fn load_trophy() -> Trophy {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/helios/onboarding.generated.md");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read trophy {}: {e}", path.display()));

    let mut current = String::new();
    let mut stale = String::new();
    let mut conflicts = String::new();
    let mut bucket: Option<&mut String> = None;
    for line in text.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            bucket = if title.starts_with("Current") {
                Some(&mut current)
            } else if title.starts_with("Stale") {
                Some(&mut stale)
            } else if title.starts_with("Conflicts") {
                Some(&mut conflicts)
            } else {
                None
            };
            continue;
        }
        if let Some(buf) = bucket.as_mut() {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    Trophy {
        current,
        stale,
        conflicts,
    }
}

#[test]
fn helios_trophy_encodes_five_of_five() {
    let t = load_trophy();

    // Sanity: all three sections exist and are non-empty (guards a malformed or
    // truncated trophy that would make the membership checks vacuous).
    assert!(
        !t.current.trim().is_empty(),
        "Current section missing/empty"
    );
    assert!(!t.stale.trim().is_empty(), "Stale section missing/empty");
    assert!(
        !t.conflicts.trim().is_empty(),
        "Conflicts section missing/empty"
    );

    // 1. Deploy day: Tuesday current; Friday and Wednesday retired.
    assert!(
        t.current.contains("Deploys moved to Tuesday"),
        "Tuesday must be the current deploy day"
    );
    assert!(
        t.stale.contains("Deploys happen on Friday"),
        "Friday deploy must be stale"
    );
    assert!(
        t.stale.contains("Deploys moved to Wednesday"),
        "Wednesday deploy must be stale"
    );
    assert!(
        !t.current.contains("Deploys happen on Friday")
            && !t.current.contains("Deploys moved to Wednesday"),
        "no retired deploy day may be current"
    );

    // 2. Approval: Bob current; Alice retired.
    assert!(
        t.current.contains("Bob owns release approval now"),
        "Bob must be the current approver"
    );
    assert!(
        t.stale.contains("Alice owns release approval"),
        "Alice approval must be stale"
    );

    // 3. Storage: BatPak represented as current; Postgres-as-store retired.
    assert!(
        t.current.contains("BatPak"),
        "BatPak must appear in current storage claims"
    );
    assert!(
        t.stale.contains("The platform uses Postgres for storage"),
        "Postgres-as-primary-store must be stale"
    );

    // 4. Release schedule: the Monday-vs-Friday conflict must surface.
    assert!(
        t.conflicts.contains("Releases happen on Monday")
            && t.conflicts.contains("Releases go out on Friday"),
        "Monday-vs-Friday release conflict must be present"
    );
}
