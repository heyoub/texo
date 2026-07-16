use super::*;

/// Verdict depends only on the text pair; call count exposes duplicate model
/// calls for identical-text pairs.
struct TextKeyedCountingRelater {
    calls: std::sync::atomic::AtomicUsize,
}

impl TextKeyedCountingRelater {
    fn new() -> Self {
        Self {
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }
    fn count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl ClaimRelater for TextKeyedCountingRelater {
    fn relate(&self, _older: &str, newer: &str) -> Result<RelationVerdict, SemanticsError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Deterministic in the text pair: "supersede" wording => Supersedes.
        let relation = if newer.contains("moved") {
            ClaimRelation::Supersedes
        } else {
            ClaimRelation::Unrelated
        };
        Ok(RelationVerdict {
            relation,
            score: 1.0,
        })
    }
    fn fingerprint(&self) -> String {
        "text-keyed-counting".to_owned()
    }
}

/// Two clusters, each three same-day claims: many intra-cluster pairs feed
/// the fan-out so worker interleaving is genuinely exercised.
fn parallel_fanout_corpus() -> (Vec<(ClaimId, ClaimView)>, FixedEmbedder) {
    let claims = vec![
        claim(
            "claim_0000000000a1",
            "deploy",
            "Deploys happen on Friday",
            1,
        ),
        claim(
            "claim_0000000000a2",
            "deploy",
            "Deploys moved to Wednesday",
            2,
        ),
        claim(
            "claim_0000000000a3",
            "deploy",
            "Deploys moved to Tuesday",
            3,
        ),
        claim(
            "claim_0000000000b1",
            "release",
            "Releases happen on Monday",
            4,
        ),
        claim(
            "claim_0000000000b2",
            "release",
            "Releases moved to Thursday",
            5,
        ),
        claim(
            "claim_0000000000b3",
            "release",
            "Releases moved to Saturday",
            6,
        ),
    ];
    let embedder = FixedEmbedder::new(
        vec![
            ("deploys happen on friday", vec![1.0, 0.0]),
            ("deploys moved to wednesday", vec![0.999, 0.01]),
            ("deploys moved to tuesday", vec![0.998, 0.02]),
            ("releases happen on monday", vec![0.0, 1.0]),
            ("releases moved to thursday", vec![0.01, 0.999]),
            ("releases moved to saturday", vec![0.02, 0.998]),
        ],
        2,
    );
    (claims, embedder)
}

#[test]
fn parallel_relate_equals_sequential_output() {
    let (claims, embedder) = parallel_fanout_corpus();
    let seq = relate_claims_with_settled(
        &claims,
        &embedder,
        &TextKeyedCountingRelater::new(),
        th(0.9, 0.6),
        &BTreeMap::new(),
        Duration::MAX,
    )
    .expect("sequential");
    for concurrency in [2_usize, 4, 8] {
        let par = relate_claims_settled_parallel(
            &claims,
            &embedder,
            &TextKeyedCountingRelater::new(),
            th(0.9, 0.6),
            &BTreeMap::new(),
            Duration::MAX,
            concurrency,
        )
        .expect("parallel");
        assert_eq!(
            format!("{seq:?}"),
            format!("{par:?}"),
            "parallel output diverged at concurrency {concurrency}"
        );
    }
}

#[test]
fn parallel_relate_coalesces_duplicate_text_pairs_to_one_call() {
    // Four logical claim ids but only two distinct text pairs among the
    // candidate pairs: identical texts must judge exactly once.
    let claims = vec![
        claim("claim_00000000dup1", "x", "The service uses Postgres", 1),
        claim("claim_00000000dup2", "x", "The service uses Postgres", 2),
        claim("claim_00000000dup3", "x", "The service uses Redis", 3),
        claim("claim_00000000dup4", "x", "The service uses Redis", 4),
    ];
    let embedder = FixedEmbedder::new(
        vec![
            ("the service uses postgres", vec![1.0, 0.0]),
            ("the service uses redis", vec![0.99, 0.02]),
        ],
        2,
    );
    let relater = TextKeyedCountingRelater::new();
    let out = relate_claims_settled_parallel(
        &claims,
        &embedder,
        &relater,
        th(0.9, 0.6),
        &BTreeMap::new(),
        Duration::MAX,
        8,
    )
    .expect("parallel");
    // All four claims cluster (postgres ~= redis at 0.9998). The two
    // same-text pairs (postgres,postgres) and (redis,redis) are dropped by
    // the normalized-text guard, leaving four distinct LOGICAL cross-pairs
    // that all carry the identical (postgres, redis) text pair. Coalescing
    // must judge that text pair exactly ONCE and fan the verdict to all
    // four — without it a cold parallel run would make four paid calls.
    assert_eq!(
        relater.count(),
        1,
        "identical-text logical pairs must coalesce to a single model call"
    );
    assert_eq!(
        out.judgments().len(),
        4,
        "every surviving logical pair receives the shared verdict"
    );
    assert!(out.unresolved().is_empty());

    // Sanity: the sequential path with the same stub (no disk cache) makes
    // one call per pair — proving the parallel coalescing, not the corpus,
    // is what collapses the calls.
    let seq_relater = TextKeyedCountingRelater::new();
    let _ = relate_claims_with_settled(
        &claims,
        &embedder,
        &seq_relater,
        th(0.9, 0.6),
        &BTreeMap::new(),
        Duration::MAX,
    )
    .expect("sequential");
    assert_eq!(
        seq_relater.count(),
        4,
        "uncached sequential path judges each of the four logical pairs"
    );
}

#[test]
fn parallel_relate_preserves_settlement_and_holdback() {
    let (claims, embedder) = parallel_fanout_corpus();
    // Pre-settle one pair from journal authority; it must not be re-judged.
    let mut settled = BTreeMap::new();
    settled.insert(
        (claims[0].0.clone(), claims[1].0.clone()),
        RelationVerdict {
            relation: ClaimRelation::Supersedes,
            score: 1.0,
        },
    );
    let relater = TextKeyedCountingRelater::new();
    let out = relate_claims_settled_parallel(
        &claims,
        &embedder,
        &relater,
        th(0.9, 0.6),
        &settled,
        Duration::MAX,
        4,
    )
    .expect("parallel");
    assert_eq!(
        out.judgments()
            .iter()
            .filter(|judgment| judgment.reused_authority)
            .count(),
        1,
        "the settled pair is reused, not re-judged"
    );
}

/// A relater that forces calls to complete in reverse dispatch order. If
/// reduction depended on completion order the output would flip; proving
/// byte-identity under this adversarial schedule witnesses the fan-out's
/// deterministic reassembly contract.
struct ReverseOrderRelater {
    gate: std::sync::Barrier,
    ranks: BTreeMap<(String, String), usize>,
    state: std::sync::Mutex<ReverseOrderState>,
    ready: std::sync::Condvar,
}

struct ReverseOrderState {
    next_rank: Option<usize>,
    completed: Vec<usize>,
}

impl ReverseOrderRelater {
    fn new(ordered_pairs: Vec<(String, String)>) -> Self {
        let total = ordered_pairs.len();
        let ranks = ordered_pairs
            .into_iter()
            .enumerate()
            .map(|(rank, pair)| (pair, rank))
            .collect();
        Self {
            gate: std::sync::Barrier::new(total),
            ranks,
            state: std::sync::Mutex::new(ReverseOrderState {
                next_rank: total.checked_sub(1),
                completed: Vec::with_capacity(total),
            }),
            ready: std::sync::Condvar::new(),
        }
    }

    fn completed(&self) -> Vec<usize> {
        self.state
            .lock()
            .expect("reverse-order state lock")
            .completed
            .clone()
    }
}

impl ClaimRelater for ReverseOrderRelater {
    fn relate(&self, older: &str, newer: &str) -> Result<RelationVerdict, SemanticsError> {
        let rank = *self
            .ranks
            .get(&(older.to_string(), newer.to_string()))
            .expect("pair has a dispatch rank");
        // All representatives must be in flight before the highest rank is
        // released. Each completion then unlocks exactly the preceding
        // rank, forcing N-1..0 without sleeps or scheduler assumptions.
        self.gate.wait();
        let mut state = self.state.lock().expect("reverse-order state lock");
        while state.next_rank != Some(rank) {
            state = self.ready.wait(state).expect("reverse-order wait");
        }
        state.completed.push(rank);
        state.next_rank = rank.checked_sub(1);
        self.ready.notify_all();
        drop(state);
        let relation = if newer.contains("moved") {
            ClaimRelation::Supersedes
        } else {
            ClaimRelation::Unrelated
        };
        Ok(RelationVerdict {
            relation,
            score: 1.0,
        })
    }
    fn fingerprint(&self) -> String {
        "reverse-order".to_owned()
    }
}

#[test]
fn parallel_reassembly_is_independent_of_completion_order() {
    let (claims, embedder) = parallel_fanout_corpus();
    let seq = relate_claims_with_settled(
        &claims,
        &embedder,
        &TextKeyedCountingRelater::new(),
        th(0.9, 0.6),
        &BTreeMap::new(),
        Duration::MAX,
    )
    .expect("sequential");

    let pending = prepare_pairs(
        &claims,
        &embedder,
        th(0.9, 0.6),
        &BTreeMap::new(),
        &RelateTemporalPolicy::default(),
        DEFAULT_CANDIDATE_PAIR_BUDGET,
        CandidateCursor::start(),
    )
    .expect("prepare")
    .expect("non-empty candidates");
    let mut seen = BTreeSet::new();
    let ordered_pairs = pending
        .pending
        .iter()
        .filter_map(|pair| {
            let texts = (
                claims[pair.old_idx].1.text.clone(),
                claims[pair.new_idx].1.text.clone(),
            );
            seen.insert(texts.clone()).then_some(texts)
        })
        .collect::<Vec<_>>();
    let representative_calls = ordered_pairs.len();
    assert!(
        representative_calls >= 2,
        "need real fan-out to test ordering"
    );

    let relater = ReverseOrderRelater::new(ordered_pairs);
    let par = relate_claims_settled_parallel(
        &claims,
        &embedder,
        &relater,
        th(0.9, 0.6),
        &BTreeMap::new(),
        Duration::MAX,
        representative_calls,
    )
    .expect("parallel");
    assert_eq!(
        format!("{seq:?}"),
        format!("{par:?}"),
        "reassembly must not depend on worker completion order"
    );
    assert_eq!(
        relater.completed(),
        (0..representative_calls).rev().collect::<Vec<_>>(),
        "test harness must actually force reverse completion order"
    );
}
