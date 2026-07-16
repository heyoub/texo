use super::*;

fn three_cluster_corpus() -> (Vec<(ClaimId, ClaimView)>, FixedEmbedder) {
    // (distinct keyword, degrees) — no keyword is a substring of another.
    let spec: [(&str, f32); 9] = [
        ("aardvark", 0.0),
        ("abacus", 1.0),
        ("acorn", 2.0),
        ("baboon", 30.0),
        ("badger", 31.0),
        ("bagel", 32.0),
        ("cactus", 60.0),
        ("camel", 61.0),
        ("candle", 62.0),
    ];
    let table: Vec<(&'static str, Vec<f32>)> = spec
        .iter()
        .map(|(key, deg)| {
            let rad = deg.to_radians();
            (*key, vec![rad.cos(), rad.sin()])
        })
        .collect();
    let claims: Vec<(ClaimId, ClaimView)> = spec
        .iter()
        .enumerate()
        .map(|(idx, (key, _))| {
            let seq = u64::try_from(idx).expect("small index") + 1;
            let id = format!("claim_{idx:012x}");
            claim(&id, "x", &format!("the {key} subject"), seq)
        })
        .collect();
    (claims, FixedEmbedder::new(table, 2))
}

const CORPUS_THRESHOLDS: RelateThresholds = RelateThresholds {
    cluster: 0.98,
    prefilter: 0.6,
};

#[test]
fn raw_candidate_page_bounds_total_pair_work_and_volume() {
    let (claims, embedder) = three_cluster_corpus();
    let relater = CountingRelater::new();
    let out = relate_claims_settled_parallel_temporal(
        &claims,
        &embedder,
        &relater,
        CORPUS_THRESHOLDS,
        &BTreeMap::new(),
        ParallelRelateOptions {
            temporal: &RelateTemporalPolicy::default(),
            budget: Duration::MAX,
            concurrency: 4,
            candidate_pair_budget: 5,
            candidate_cursor: CandidateCursor::start(),
        },
    )
    .expect("bounded page");
    let partial = partial(&out).expect("nine claims require more than one page");
    assert_eq!(partial.candidate_pairs, 5);
    assert_eq!(partial.next_candidate_cursor.offset(), 5);
    assert!(partial.judgments.len() <= 5);
    assert!(relater.count() <= 5);
}

#[test]
fn clustering_is_deterministic_across_runs() {
    let (claims, embedder) = three_cluster_corpus();
    let first = group_claims(&claims, &embedder, CORPUS_THRESHOLDS.cluster).expect("group");
    for _ in 0..5 {
        let again = group_claims(&claims, &embedder, CORPUS_THRESHOLDS.cluster).expect("group");
        assert_eq!(first, again, "same input must yield identical clusters");
    }
    // Stable ordering: members ascend within a group; groups are ordered by
    // their first (smallest) member index — never hash-map iteration order.
    for group in &first {
        assert!(group.windows(2).all(|w| w[0] < w[1]));
    }
    let firsts: Vec<usize> = first.iter().map(|g| g[0]).collect();
    assert!(firsts.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn relate_is_deterministic_across_runs() {
    let (claims, embedder) = three_cluster_corpus();
    // Script a supersession and a conflict inside two different clusters so
    // both output vectors are non-empty.
    let relater = ScriptedRelater::new(vec![
        ("aardvark", "acorn", ClaimRelation::Supersedes),
        ("baboon", "bagel", ClaimRelation::Conflict),
    ]);
    let first = relate_claims(&claims, &embedder, &relater, CORPUS_THRESHOLDS).expect("relate");
    assert_eq!(related(&first).expect("complete").supersessions.len(), 1);
    assert_eq!(related(&first).expect("complete").conflicts.len(), 1);
    for _ in 0..5 {
        let again = relate_claims(&claims, &embedder, &relater, CORPUS_THRESHOLDS).expect("relate");
        assert_eq!(
            related(&first).expect("complete").supersessions,
            related(&again).expect("complete").supersessions,
            "supersessions must be identical across runs"
        );
        let ids = |out: &RelateOutcome| -> Vec<String> {
            related(out)
                .expect("complete")
                .conflicts
                .iter()
                .map(|c| c.conflict_id.to_string())
                .collect()
        };
        assert_eq!(
            ids(&first),
            ids(&again),
            "conflict ids must be identical across runs"
        );
    }
}

#[test]
fn unresolved_pair_taints_and_holds_dependent_supersession() {
    struct PartiallyFailingRelater;
    impl ClaimRelater for PartiallyFailingRelater {
        fn relate(&self, older: &str, newer: &str) -> Result<RelationVerdict, SemanticsError> {
            if older.contains("alpha") && newer.contains("beta") {
                return Err(SemanticsError::Backend {
                    source: Box::new(std::io::Error::other("judge failed")),
                });
            }
            Ok(RelationVerdict {
                relation: if older.contains("alpha") && newer.contains("gamma") {
                    ClaimRelation::Supersedes
                } else {
                    ClaimRelation::Unrelated
                },
                score: 1.0,
            })
        }

        fn fingerprint(&self) -> String {
            "partial".to_string()
        }
    }

    let claims = vec![
        claim("claim_aaaaaaaaaaaa", "x", "alpha old", 1),
        claim("claim_bbbbbbbbbbbb", "x", "beta uncertain", 2),
        claim("claim_cccccccccccc", "x", "gamma winner", 3),
    ];
    let embedder = FixedEmbedder::new(
        vec![
            ("alpha", vec![1.0, 0.0]),
            ("beta", vec![1.0, 0.0]),
            ("gamma", vec![1.0, 0.0]),
        ],
        2,
    );
    let outcome = relate_claims(&claims, &embedder, &PartiallyFailingRelater, th(0.9, 0.9))
        .expect("partial outcome");
    let outcome = partial(&outcome).expect("partial");
    assert_eq!(outcome.unresolved.len(), 1);
    assert!(matches!(
        outcome.held.as_slice(),
        [HeldDecision::Supersession { old_claim, new_claim, .. }]
            if old_claim.as_str() == "claim_aaaaaaaaaaaa"
                && new_claim.as_str() == "claim_cccccccccccc"
    ));
}

#[test]
fn authoritative_pair_is_reused_without_judge_call() {
    let claims = vec![
        claim("claim_aaaaaaaaaaaa", "x", "alpha", 1),
        claim("claim_bbbbbbbbbbbb", "x", "beta", 2),
        claim("claim_cccccccccccc", "x", "gamma", 3),
    ];
    let embedder = FixedEmbedder::new(
        vec![
            ("alpha", vec![1.0, 0.0]),
            ("beta", vec![1.0, 0.0]),
            ("gamma", vec![1.0, 0.0]),
        ],
        2,
    );
    let relater = CountingRelater::new();
    let mut settled = BTreeMap::new();
    settled.insert(
        (claims[0].0.clone(), claims[1].0.clone()),
        RelationVerdict {
            relation: ClaimRelation::Unrelated,
            score: 1.0,
        },
    );
    let outcome = relate_claims_with_settled(
        &claims,
        &embedder,
        &relater,
        th(0.9, 0.9),
        &settled,
        Duration::MAX,
    )
    .expect("outcome");
    assert_eq!(relater.count(), 2);
    assert_eq!(
        outcome
            .judgments()
            .iter()
            .filter(|judgment| judgment.reused_authority)
            .count(),
        1
    );
}
