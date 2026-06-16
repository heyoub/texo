//! END-TO-END CAPSTONE (live): the "1/5 -> 5/5" proof.
//!
//! Extracts the real, deliberately-messy Helios corpus with the OpenRouter
//! proposer (Stage 0->1->2), then relates the claims with the real gemini
//! embedder + nemotron relater, and asserts the ground-truth oracle: the deploy
//! day is Tuesday (Friday and Wednesday superseded), Bob owns approval (Alice
//! superseded), storage is BatPak (Postgres superseded), and Monday-vs-Friday is
//! a live release conflict.
//!
//! `#[ignore]`d and key-gated: skips cleanly unless `OPENROUTER_API_KEY` is set.
//! Run: `cargo test -p texo-extract --test helios_e2e -- --ignored --nocapture`.

use std::collections::HashSet;
use std::path::PathBuf;

use texo_core::types::ids::SourceId;
use texo_core::types::receipt::receipt_view;
use texo_core::{relate_claims, ClaimId, ClaimStatus, ClaimView, DEFAULT_GROUNDING_THRESHOLD_PPM};
use texo_extract::run_extraction;
use texo_semantics::{OpenRouterEmbedder, OpenRouterProposer, OpenRouterRelater};

/// Coarse cosine prefilter for relating (below the lowest true same-subject
/// similarity; the relater does the real separation).
const PREFILTER: f32 = 0.60;

fn docs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/helios/docs")
}

/// Build a `ClaimView` for an extracted claim. `seq` is the global journal
/// sequence (recency: later doc/line = higher = newer).
fn build_view(
    seq: u64,
    source_path: &str,
    line_start: u32,
    text: &str,
    normalized: &str,
    confidence_ppm: u32,
) -> (ClaimId, ClaimView) {
    let id_str = format!("claim_{seq:012x}");
    let id = ClaimId::try_from(id_str.as_str()).expect("valid claim id");
    let view = ClaimView {
        claim_id: id.clone(),
        workspace_id: "helios".to_string(),
        source_id: SourceId::try_from("src_abc123def456").expect("valid source id"),
        source_path: source_path.to_string(),
        line_start,
        line_end: line_start,
        text: text.to_string(),
        normalized_text: normalized.to_string(),
        subject_hint: "extracted".to_string(),
        predicate_hint: "extracted".to_string(),
        object_hint: String::new(),
        confidence_ppm,
        extractor_kind: "or:opus-4.8".to_string(),
        status: ClaimStatus::Current,
        receipt: receipt_view(
            u128::from(seq),
            seq,
            "ClaimRecorded",
            "workspace:helios",
            &id_str,
        ),
        supersedes: Vec::new(),
        superseded_by: None,
    };
    (id, view)
}

/// Stage 0->1->2 over the whole corpus, in chronological (filename) order, with
/// content-addressed dedup (identical claims collapse to one, as the journal does).
fn extract_corpus() -> Vec<(ClaimId, ClaimView)> {
    let proposer = OpenRouterProposer::new(None).expect("proposer");
    let mut files: Vec<PathBuf> = std::fs::read_dir(docs_dir())
        .expect("read docs dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    files.sort();

    let mut claims: Vec<(ClaimId, ClaimView)> = Vec::new();
    let mut seen_text: HashSet<String> = HashSet::new();
    let mut seq: u64 = 0;
    for file in &files {
        let source = std::fs::read_to_string(file).expect("read doc");
        let extracted =
            run_extraction(&source, &proposer, DEFAULT_GROUNDING_THRESHOLD_PPM).expect("extract");
        let path = file
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        for oc in extracted {
            if !seen_text.insert(oc.normalized_text.clone()) {
                continue;
            }
            seq += 1;
            claims.push(build_view(
                seq,
                &path,
                oc.line_start,
                &oc.text,
                &oc.normalized_text,
                oc.confidence_ppm,
            ));
        }
    }
    eprintln!("extracted {} distinct claims", claims.len());
    claims
}

#[test]
#[ignore = "live end-to-end OpenRouter run; requires OPENROUTER_API_KEY"]
fn helios_corpus_reaches_five_of_five() {
    if std::env::var("OPENROUTER_API_KEY").is_err() {
        eprintln!("SKIP: OPENROUTER_API_KEY not set");
        return;
    }

    let claims = extract_corpus();

    let embedder = OpenRouterEmbedder::new(None).expect("embedder");
    let relater = OpenRouterRelater::new(None).expect("relater");
    let out = relate_claims(&claims, &embedder, &relater, PREFILTER).expect("relate");

    let text_of = |id: &ClaimId| -> String {
        claims
            .iter()
            .find(|(cid, _)| cid == id)
            .map(|(_, v)| v.text.clone())
            .unwrap_or_default()
    };

    eprintln!("\n--- supersessions (old -> new) ---");
    for (o, n, _) in &out.supersessions {
        eprintln!("  {:?}  ->  {:?}", text_of(o), text_of(n));
    }
    eprintln!("--- conflicts ---");
    for c in &out.conflicts {
        eprintln!("  {:?}  <>  {:?}", text_of(&c.claim_a), text_of(&c.claim_b));
    }
    eprintln!();

    let superseded: HashSet<&ClaimId> = out.supersessions.iter().map(|(o, _, _)| o).collect();
    // End-state predicates — what the generated onboarding would actually show.
    // We assert the *current set* rather than exact supersession edges: the messy
    // corpus admits several valid edges to the same end-state, so the user-visible
    // outcome (which claim is current) is the meaningful, robust invariant.
    let exists = |sub: &str| claims.iter().any(|(_, v)| v.text.contains(sub));
    let is_current = |sub: &str| {
        claims
            .iter()
            .any(|(id, v)| v.text.contains(sub) && !superseded.contains(id))
    };
    // Retired: the claim was extracted but no copy of it remains current.
    let retired = |sub: &str| exists(sub) && !is_current(sub);
    // Topic-level: some *current* claim mentions `needle`. Used where a fact is
    // told across several claims (e.g. the BatPak migration) and any of them
    // being current correctly represents the topic in the onboarding.
    let current_mentions = |needle: &str| {
        claims
            .iter()
            .any(|(id, v)| !superseded.contains(id) && v.text.contains(needle))
    };

    // 1. Deploy day: Tuesday is current; Friday and Wednesday are retired.
    assert!(
        is_current("Deploys moved to Tuesday"),
        "Tuesday must be the current deploy day"
    );
    assert!(
        retired("Deploys happen on Friday"),
        "Friday deploy must be retired (superseded)"
    );
    assert!(
        retired("Deploys moved to Wednesday"),
        "Wednesday deploy must be retired (superseded)"
    );

    // 2. Approval owner: Bob is current; Alice is retired.
    assert!(
        is_current("Bob owns release approval now"),
        "Bob must be the current approval owner"
    );
    assert!(
        retired("Alice owns release approval"),
        "Alice approval must be retired (superseded)"
    );

    // 3. Storage: BatPak is represented in the current set; the Postgres-as-
    //    primary-store claim is retired. (The migration is told across several
    //    claims, so topic-level currency is the meaningful invariant here.)
    assert!(
        current_mentions("BatPak"),
        "BatPak must appear in the current storage claims"
    );
    assert!(
        retired("uses Postgres for storage"),
        "Postgres-as-primary-store must be retired (superseded)"
    );

    // 4. Release schedule: Monday vs Friday is a live conflict (both current).
    let has_release_conflict = out.conflicts.iter().any(|c| {
        let (a, b) = (text_of(&c.claim_a), text_of(&c.claim_b));
        (a.contains("Releases happen on Monday") && b.contains("go out on Friday"))
            || (b.contains("Releases happen on Monday") && a.contains("go out on Friday"))
    });
    assert!(
        has_release_conflict,
        "Monday vs Friday release-schedule conflict must surface"
    );

    eprintln!("OK: Helios corpus reached 5/5 — real extraction + real relation.");
}
