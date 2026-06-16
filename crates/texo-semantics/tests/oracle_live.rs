//! PROVES (live): the real OpenRouter gemini embeddings + LLM-judge NLI reproduce,
//! on the actual Helios claim sentences, the same relations the scripted-stub unit
//! tests assert in texo-core's semantics_pipeline. This is the Phase 2 capstone — it
//! validates that the hosted models, not just the logic, get the oracle right.
//!
//! It is `#[ignore]`d and skips cleanly unless `OPENROUTER_API_KEY` is set, so it never
//! runs in the default gate. Run it with:
//!   OPENROUTER_API_KEY=... cargo test -p texo-semantics --test oracle_live -- --ignored --nocapture

use texo_core::types::ids::SourceId;
use texo_core::types::receipt::receipt_view;
use texo_core::{relate_claims, ClaimId, ClaimStatus, ClaimView};
use texo_semantics::{OpenRouterEmbedder, OpenRouterRelater};

/// Coarse cosine **prefilter** for gemini-embedding-2: pairs below this are never
/// sent to the relation judge. It must sit *below* the lowest same-subject
/// similarity in the corpus (measured: Postgres↔BatPak ≈ 0.70) so no true pair is
/// dropped — the relater, not this threshold, does the subject separation.
const PREFILTER: f32 = 0.60;

/// Build a claim. `seq` is the journal sequence (recency: higher = newer) and also the
/// unique id seed; `text` is the claim sentence; `src` the originating doc.
fn claim(seq: u64, text: &str, src: &str) -> (ClaimId, ClaimView) {
    let id_str = format!("claim_{seq:012x}");
    let id = ClaimId::try_from(id_str.as_str()).expect("valid claim id");
    let view = ClaimView {
        claim_id: id.clone(),
        workspace_id: "helios".to_string(),
        source_id: SourceId::try_from("src_abc123def456").expect("valid source id"),
        source_path: src.to_string(),
        line_start: 1,
        line_end: 1,
        text: text.to_string(),
        normalized_text: text.to_ascii_lowercase(),
        subject_hint: "unused-by-semantic-path".to_string(),
        predicate_hint: "unknown".to_string(),
        object_hint: String::new(),
        confidence_ppm: 650_000,
        extractor_kind: "test".to_string(),
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

#[test]
#[ignore = "live OpenRouter call; requires OPENROUTER_API_KEY"]
fn helios_relations_hold_with_real_models() {
    if std::env::var("OPENROUTER_API_KEY").is_err() {
        eprintln!("SKIP: OPENROUTER_API_KEY not set");
        return;
    }

    // The real Helios claim sentences, with recency via sequence numbers.
    let claims = vec![
        claim(10, "Deploys happen on Friday.", "02_adr_001.md"),
        claim(20, "Deploys moved to Wednesday.", "03_adr_007.md"),
        claim(30, "Deploys moved to Tuesday.", "04_runbook.md"),
        claim(11, "Alice owns release approval.", "02_adr_001.md"),
        claim(40, "Bob owns release approval now.", "05_meeting.md"),
        claim(21, "Releases happen on Monday.", "04_runbook.md"),
        claim(41, "Releases go out on Friday.", "06_rogue.md"),
        claim(
            12,
            "The platform uses Postgres for storage.",
            "01_onboarding.md",
        ),
        claim(
            42,
            "The platform uses BatPak for append-only event storage now.",
            "07_adr_019.md",
        ),
        claim(
            50,
            "dave asked about the deploy day, bob said check the runbook.",
            "05_meeting.md",
        ),
    ];
    let text_of = |id: &ClaimId| -> String {
        claims
            .iter()
            .find(|(cid, _)| cid == id)
            .map(|(_, v)| v.text.clone())
            .unwrap_or_default()
    };

    let embedder = OpenRouterEmbedder::new(None).expect("embedder");
    let relater = OpenRouterRelater::new(None).expect("relater");

    let out = relate_claims(&claims, &embedder, &relater, PREFILTER).expect("relate");
    let edges = &out.supersessions;
    let conflicts = &out.conflicts;

    eprintln!("\n--- supersession edges (old -> new) ---");
    for (old, new, why) in edges {
        eprintln!("  {:?}  ->  {:?}   [{why}]", text_of(old), text_of(new));
    }

    let edge = |old_sub: &str, new_sub: &str| {
        edges
            .iter()
            .any(|(o, n, _)| text_of(o).contains(old_sub) && text_of(n).contains(new_sub))
    };

    // Headline supersessions the real models must produce.
    assert!(
        edge("Deploys happen on Friday", "moved to Tuesday"),
        "Friday -> Tuesday"
    );
    assert!(
        edge("moved to Wednesday", "moved to Tuesday"),
        "Wednesday -> Tuesday"
    );
    assert!(edge("Alice owns", "Bob owns"), "Alice -> Bob");
    assert!(edge("uses Postgres", "uses BatPak"), "Postgres -> BatPak");

    // TRAP 1: the meeting-noise line must NOT supersede the real deploy decision.
    assert!(
        !edges
            .iter()
            .any(|(_, n, _)| text_of(n).contains("dave asked")),
        "noise line must never be the superseding (new) side"
    );

    // TRAP 2: approval-owner and release-schedule are different subjects — no cross edge.
    let crosses_approval_schedule = |a: &str, b: &str| text_of_pair_crosses(edges, &text_of, a, b);
    assert!(
        !crosses_approval_schedule("owns release approval", "Releases"),
        "approval-owner must not link to release-schedule via supersession"
    );

    eprintln!("\n--- conflicts ---");
    for c in conflicts {
        eprintln!("  {:?}  <>  {:?}", text_of(&c.claim_a), text_of(&c.claim_b));
    }

    // The release-schedule conflict (Monday vs Friday, both current) must surface.
    let has_release_conflict = conflicts.iter().any(|c| {
        let (a, b) = (text_of(&c.claim_a), text_of(&c.claim_b));
        (a.contains("Releases happen on Monday") && b.contains("go out on Friday"))
            || (b.contains("Releases happen on Monday") && a.contains("go out on Friday"))
    });
    assert!(
        has_release_conflict,
        "Monday vs Friday release-schedule conflict"
    );

    // TRAP 2 again on the conflict side: approval pair must not be a conflict.
    assert!(
        !conflicts.iter().any(|c| {
            let (a, b) = (text_of(&c.claim_a), text_of(&c.claim_b));
            (a.contains("owns release approval") && b.contains("Releases"))
                || (b.contains("owns release approval") && a.contains("Releases"))
        }),
        "approval-owner must not conflict with release-schedule"
    );

    eprintln!("\nOK: real OpenRouter models reproduced the Helios relations.");
}

/// True if any edge links a claim containing `a` with one containing `b` (either side).
fn text_of_pair_crosses(
    edges: &[(ClaimId, ClaimId, String)],
    text_of: &impl Fn(&ClaimId) -> String,
    a: &str,
    b: &str,
) -> bool {
    edges.iter().any(|(o, n, _)| {
        let (to, tn) = (text_of(o), text_of(n));
        (to.contains(a) && tn.contains(b)) || (to.contains(b) && tn.contains(a))
    })
}
