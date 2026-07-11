#![cfg(feature = "openrouter")]

//! Live oracle smoke test for hosted `OpenRouter` models.

use texo::events::ids::{ClaimId, SourceId};
use texo::semantics::openrouter::{OpenRouterEmbedder, OpenRouterRelater};
use texo::semantics::pipeline::{
    receipt_view, relate_claims, ClaimStatus, ClaimView, RelateThresholds,
};

/// Cluster-first thresholds for gemini-embedding-2, mirroring `texo relate`.
const THRESHOLDS: RelateThresholds = RelateThresholds {
    cluster: 0.65,
    prefilter: 0.60,
};

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
#[ignore = "live model call; requires TEXO_LLM_API_KEY"]
fn helios_relations_hold_with_real_models() {
    if std::env::var("TEXO_LLM_API_KEY").is_err() {
        return;
    }

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

    let embedder = OpenRouterEmbedder::new(None, None).expect("embedder");
    let relater = OpenRouterRelater::new(None, None).expect("relater");

    let out = relate_claims(&claims, &embedder, &relater, THRESHOLDS).expect("relate");
    let edges = &out.supersessions;
    let conflicts = &out.conflicts;

    let edge = |old_sub: &str, new_sub: &str| {
        edges
            .iter()
            .any(|(old, new, _)| text_of(old).contains(old_sub) && text_of(new).contains(new_sub))
    };

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

    assert!(
        !edges
            .iter()
            .any(|(_, new, _)| text_of(new).contains("dave asked")),
        "noise line must never be the superseding side"
    );

    let crosses_approval_schedule = |a: &str, b: &str| text_of_pair_crosses(edges, &text_of, a, b);
    assert!(
        !crosses_approval_schedule("owns release approval", "Releases"),
        "approval-owner must not link to release-schedule via supersession"
    );

    let has_release_conflict = conflicts.iter().any(|conflict| {
        let (a, b) = (text_of(&conflict.claim_a), text_of(&conflict.claim_b));
        (a.contains("Releases happen on Monday") && b.contains("go out on Friday"))
            || (b.contains("Releases happen on Monday") && a.contains("go out on Friday"))
    });
    assert!(
        has_release_conflict,
        "Monday vs Friday release-schedule conflict"
    );

    assert!(
        !conflicts.iter().any(|conflict| {
            let (a, b) = (text_of(&conflict.claim_a), text_of(&conflict.claim_b));
            (a.contains("owns release approval") && b.contains("Releases"))
                || (b.contains("owns release approval") && a.contains("Releases"))
        }),
        "approval-owner must not conflict with release-schedule"
    );
}

fn text_of_pair_crosses(
    edges: &[(ClaimId, ClaimId, String)],
    text_of: &impl Fn(&ClaimId) -> String,
    a: &str,
    b: &str,
) -> bool {
    edges.iter().any(|(old, new, _)| {
        let (old_text, new_text) = (text_of(old), text_of(new));
        (old_text.contains(a) && new_text.contains(b))
            || (old_text.contains(b) && new_text.contains(a))
    })
}
