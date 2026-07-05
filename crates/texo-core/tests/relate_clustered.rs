//! PROVES: cluster-first relate on a REAL BatPak store (no test doubles).
//!
//! Ingests two markdown sessions into a real journal, replays, runs
//! [`relate_claims`] with deterministic in-test backends (pure trait stubs — the
//! store, journal, replay, and events are all real), and asserts:
//!
//! * the LLM judge is only ever called for *within-cluster* pairs — the
//!   O(n²)-fix bound `Σ (|cluster| choose 2)`, strictly fewer calls than the
//!   coarse prefilter alone would admit;
//! * the relate output is deterministic across repeated runs over the same
//!   replayed state;
//! * the journaled `ClaimSuperseded` / `ClaimConflictDetected` events replay to
//!   the expected statuses after close/reopen (record-once boundary intact).

mod support;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use support::{setup_demo_journal, temp_workspace};
use texo_core::{
    cosine_similarity, group_claims, ingest_sources, open_journal, relate_claims,
    ClaimConflictDetected, ClaimId, ClaimRelater, ClaimRelation, ClaimStatus, ClaimSuperseded,
    ClaimView, Embedder, IngestMode, RelateThresholds, RelatedClaims, RelationVerdict,
    SemanticsError, FIXTURE_OBSERVED_AT_MS,
};

const THRESHOLDS: RelateThresholds = RelateThresholds {
    cluster: 0.9,
    prefilter: 0.6,
};

/// Deterministic embedder keyed on a distinctive token of each claim's
/// normalized text. Vectors are 2-D unit vectors at fixed angles: the storage
/// pair and the queue pair are tight (1° apart, cosine ≈ 0.9998), the storage
/// and queue clusters sit 45° apart (cosine ≈ 0.71 — above the 0.6 prefilter,
/// below the 0.9 cluster threshold), and the frontend claim is orthogonal to
/// storage.
struct AngleEmbedder;

impl AngleEmbedder {
    fn degrees_for(text: &str) -> f32 {
        let lower = text.to_ascii_lowercase();
        for (key, deg) in [
            ("postgres", 0.0),
            ("batpak", 1.0),
            ("redis", 45.0),
            ("kafka", 46.0),
            ("typescript", 90.0),
        ] {
            if lower.contains(key) {
                return deg;
            }
        }
        panic!("unexpected claim text in fixture corpus: {text}");
    }
}

impl Embedder for AngleEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, SemanticsError> {
        let rad = Self::degrees_for(text).to_radians();
        Ok(vec![rad.cos(), rad.sin()])
    }
}

/// Deterministic relater: counts every judge call, scripts the storage
/// supersession and the queue conflict, and answers Unrelated otherwise.
struct CountingScriptedRelater {
    calls: AtomicUsize,
}

impl CountingScriptedRelater {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
    fn count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ClaimRelater for CountingScriptedRelater {
    fn relate(&self, older: &str, newer: &str) -> Result<RelationVerdict, SemanticsError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (o, n) = (older.to_ascii_lowercase(), newer.to_ascii_lowercase());
        let relation = if o.contains("postgres") && n.contains("batpak") {
            ClaimRelation::Supersedes
        } else if o.contains("redis") && n.contains("kafka") {
            ClaimRelation::Conflict
        } else {
            ClaimRelation::Unrelated
        };
        Ok(RelationVerdict {
            relation,
            score: 1.0,
        })
    }
    fn fingerprint(&self) -> String {
        "counting-scripted".to_owned()
    }
}

/// Write one markdown source and ingest it as its own committed session.
fn ingest_session(root: &Path, session: &str, file_name: &str, body: &str) {
    let dir = root.join(session);
    std::fs::create_dir_all(&dir).expect("mkdir session dir");
    std::fs::write(dir.join(file_name), body).expect("write source");

    let journal = open_journal(root).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    ingest_sources(
        journal.handle(),
        journal.config(),
        &workspace,
        &dir,
        IngestMode::Commit,
        FIXTURE_OBSERVED_AT_MS,
        root,
    )
    .expect("ingest");
    journal.close().expect("close");
}

/// Replay the store and return the Current claims ordered by journal sequence
/// (then id) — the same deterministic ordering `texo relate` uses.
fn current_claims_in_sequence_order(root: &Path) -> Vec<(ClaimId, ClaimView)> {
    let journal = open_journal(root).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    let replayed = journal.replay(&workspace).expect("replay");
    journal.close().expect("close");

    let mut claims: Vec<(ClaimId, ClaimView)> = replayed
        .state
        .claims
        .iter()
        .filter(|(_, view)| view.status == ClaimStatus::Current)
        .map(|(id, view)| (id.clone(), view.clone()))
        .collect();
    claims.sort_by(|a, b| {
        a.1.receipt
            .sequence
            .get()
            .cmp(&b.1.receipt.sequence.get())
            .then_with(|| a.0.as_str().cmp(b.0.as_str()))
    });
    claims
}

fn find_claim<'a>(claims: &'a [(ClaimId, ClaimView)], needle: &str) -> &'a (ClaimId, ClaimView) {
    claims
        .iter()
        .find(|(_, v)| v.normalized_text.contains(needle))
        .unwrap_or_else(|| panic!("fixture claim containing {needle:?} must exist"))
}

/// Count the pairs the pre-clustering pipeline would have judged: every pair
/// clearing the coarse prefilter, regardless of cluster.
fn prefilter_pair_count(claims: &[(ClaimId, ClaimView)], embedder: &dyn Embedder) -> usize {
    let n = claims.len();
    let mut count = 0usize;
    for i in 0..n {
        for j in (i + 1)..n {
            let a = embedder.embed(&claims[i].1.normalized_text).expect("embed");
            let b = embedder.embed(&claims[j].1.normalized_text).expect("embed");
            if cosine_similarity(&a, &b) >= THRESHOLDS.prefilter {
                count += 1;
            }
        }
    }
    count
}

/// Journal the relate output exactly as `texo relate` does (record-once).
fn journal_relations(root: &Path, out: &RelatedClaims) {
    let journal = open_journal(root).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    for (old, new, reason) in &out.supersessions {
        journal
            .handle()
            .append_superseded(&ClaimSuperseded {
                old_claim_id: old.to_string(),
                new_claim_id: new.to_string(),
                workspace_id: workspace.to_string(),
                reason: reason.clone(),
                decided_by: "texo-relate".to_string(),
                observed_at_ms: FIXTURE_OBSERVED_AT_MS,
            })
            .expect("append superseded");
    }
    for entry in &out.conflicts {
        journal
            .handle()
            .append_conflict(&ClaimConflictDetected {
                conflict_id: entry.conflict_id.to_string(),
                workspace_id: workspace.to_string(),
                claim_a: entry.claim_a.to_string(),
                claim_b: entry.claim_b.to_string(),
                reason: entry.reason.clone(),
                status: "open".to_string(),
                observed_at_ms: FIXTURE_OBSERVED_AT_MS,
            })
            .expect("append conflict");
    }
    journal.close().expect("close");
}

#[test]
fn cluster_first_relate_on_real_store_bounds_judge_calls_and_replays() {
    let dir = temp_workspace();
    let root = dir.path();
    setup_demo_journal(root);

    // Session 1 (older): the storage and queue facts as first told. Claim lines
    // carry a heuristic signal word ("uses"/"is") but no replacement keyword and
    // no shared subject hint, so ingest records them all as Current without any
    // heuristic supersession.
    ingest_session(
        root,
        "session1",
        "a_services.md",
        "# Services\n\nBilling storage uses Postgres.\n\nTask queue is Redis.\n",
    );
    // Session 2 (newer): the updated storage fact, a disagreeing queue fact, and
    // an unrelated frontend fact.
    ingest_session(
        root,
        "session2",
        "b_platform.md",
        "# Platform\n\nBilling storage uses BatPak.\n\nTask queue is Kafka.\n\nFrontend uses TypeScript.\n",
    );

    let claims = current_claims_in_sequence_order(root);
    assert_eq!(claims.len(), 5, "all five fixture claims must be Current");

    let (postgres_id, postgres_view) = find_claim(&claims, "postgres");
    let (batpak_id, batpak_view) = find_claim(&claims, "batpak");
    let (redis_id, _) = find_claim(&claims, "redis");
    let (kafka_id, _) = find_claim(&claims, "kafka");
    let (typescript_id, _) = find_claim(&claims, "typescript");
    assert!(
        postgres_view.receipt.sequence.get() < batpak_view.receipt.sequence.get(),
        "session-1 claim must be older than session-2 claim"
    );

    let embedder = AngleEmbedder;

    // Sanity: the real claims cluster as {postgres, batpak}, {redis, kafka},
    // {typescript} at the cluster threshold, so Σ (m_i choose 2) = 2.
    let groups = group_claims(&claims, &embedder, THRESHOLDS.cluster).expect("group");
    let mut sizes: Vec<usize> = groups.iter().map(Vec::len).collect();
    sizes.sort_unstable();
    assert_eq!(sizes, vec![1, 2, 2]);
    let within_cluster_pairs: usize = groups.iter().map(|g| g.len() * (g.len() - 1) / 2).sum();

    // How many pairs the pre-clustering pipeline would have judged: the four
    // storage×queue cross pairs and the two queue×frontend cross pairs all sit
    // above 0.6, so 8 in total.
    let prefilter_pairs = prefilter_pair_count(&claims, &embedder);
    assert_eq!(prefilter_pairs, 8, "fixture geometry must hold");

    // Relate: the judge must only see within-cluster pairs.
    let relater = CountingScriptedRelater::new();
    let out = relate_claims(&claims, &embedder, &relater, THRESHOLDS).expect("relate");
    assert_eq!(
        relater.count(),
        within_cluster_pairs,
        "judge calls must equal the within-cluster pair count"
    );
    assert!(
        relater.count() < prefilter_pairs,
        "clustering must judge strictly fewer pairs than the prefilter alone"
    );

    // Verdict semantics unchanged: the storage pair supersedes, the queue pair
    // conflicts, and nothing touches the frontend claim.
    assert_eq!(out.supersessions.len(), 1);
    let (old, new, _) = &out.supersessions[0];
    assert_eq!(old, postgres_id);
    assert_eq!(new, batpak_id);
    assert_eq!(out.conflicts.len(), 1);
    let conflict = &out.conflicts[0];
    let mut pair = [conflict.claim_a.as_str(), conflict.claim_b.as_str()];
    pair.sort_unstable();
    let mut expected = [redis_id.as_str(), kafka_id.as_str()];
    expected.sort_unstable();
    assert_eq!(pair, expected);

    // Determinism over the same replayed state: repeated runs are identical.
    for _ in 0..3 {
        let again = relate_claims(&claims, &embedder, &relater, THRESHOLDS).expect("relate");
        assert_eq!(out.supersessions, again.supersessions);
        let ids = |o: &RelatedClaims| -> Vec<String> {
            o.conflicts
                .iter()
                .map(|c| c.conflict_id.to_string())
                .collect()
        };
        assert_eq!(ids(&out), ids(&again));
    }

    journal_relations(root, &out);

    // Close/reopen: replay must project the journaled relations.
    let journal = open_journal(root).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    let replayed = journal.replay(&workspace).expect("replay");
    journal.close().expect("close");

    let status_of = |id: &ClaimId| replayed.state.claims[id].status;
    assert_eq!(status_of(postgres_id), ClaimStatus::Superseded);
    assert_eq!(
        replayed.state.claims[postgres_id]
            .superseded_by
            .as_ref()
            .map(ToString::to_string),
        Some(batpak_id.to_string())
    );
    for id in [batpak_id, typescript_id] {
        assert_eq!(status_of(id), ClaimStatus::Current);
    }
    // The journaled conflict flips both queue claims to Conflicting: they stay
    // visible (neither is superseded) but are flagged as disagreeing.
    for id in [redis_id, kafka_id] {
        assert_eq!(status_of(id), ClaimStatus::Conflicting);
    }
    let replayed_conflict = replayed
        .state
        .conflicts
        .get(&out.conflicts[0].conflict_id)
        .expect("journaled conflict must replay");
    let mut replayed_pair = [
        replayed_conflict.claim_a.as_str(),
        replayed_conflict.claim_b.as_str(),
    ];
    replayed_pair.sort_unstable();
    assert_eq!(replayed_pair, expected);
}
