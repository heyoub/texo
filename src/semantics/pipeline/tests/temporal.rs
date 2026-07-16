use super::*;

#[test]
fn git_ancestry_orients_pairs_instead_of_ingest_sequence() {
    let claims = vec![
        claim(
            "claim_aaaaaaaaaaaa",
            "storage",
            "old storage uses postgres",
            20,
        ),
        claim(
            "claim_bbbbbbbbbbbb",
            "storage",
            "new storage uses batpak",
            10,
        ),
    ];
    let embedder = FixedEmbedder::new(vec![("storage", vec![1.0, 0.0])], 2);
    let relater = ScriptedRelater::new(vec![(
        "old storage",
        "new storage",
        ClaimRelation::Supersedes,
    )]);
    let left = SourceSnapshotId::derive("ancestor");
    let right = SourceSnapshotId::derive("descendant");
    let mut temporal = RelateTemporalPolicy::default();
    temporal.bind_claim(&claims[0].0, &left);
    temporal.bind_claim(&claims[1].0, &right);
    temporal.insert_relation(&left, &right, TemporalRelation::Before);

    let out = relate_claims_with_settled_temporal(
        &claims,
        &embedder,
        &relater,
        th(0.9, 0.6),
        &BTreeMap::new(),
        &temporal,
        Duration::MAX,
    )
    .expect("relate");

    assert_eq!(
        related(&out).expect("complete").supersessions,
        vec![(
            claims[0].0.clone(),
            claims[1].0.clone(),
            "superseded by x.md:10".to_string(),
        )]
    );
    assert_eq!(out.judgments()[0].older_claim, claims[0].0);
    assert_eq!(out.judgments()[0].newer_claim, claims[1].0);
}

#[test]
fn incomparable_or_missing_source_order_never_calls_the_judge() {
    for (relation, expected) in [
        (
            Some(TemporalRelation::Concurrent),
            RelationFailureClass::TemporalConcurrent,
        ),
        (None, RelationFailureClass::TemporalUnknown),
    ] {
        let claims = vec![
            claim(
                "claim_aaaaaaaaaaaa",
                "storage",
                "old storage uses postgres",
                1,
            ),
            claim(
                "claim_bbbbbbbbbbbb",
                "storage",
                "new storage uses batpak",
                2,
            ),
        ];
        let embedder = FixedEmbedder::new(vec![("storage", vec![1.0, 0.0])], 2);
        let relater = CountingRelater::new();
        let left = SourceSnapshotId::derive("left");
        let right = SourceSnapshotId::derive("right");
        let mut temporal = RelateTemporalPolicy::default();
        temporal.bind_claim(&claims[0].0, &left);
        temporal.bind_claim(&claims[1].0, &right);
        if let Some(relation) = relation {
            temporal.insert_relation(&left, &right, relation);
        }

        let out = relate_claims_settled_parallel_temporal(
            &claims,
            &embedder,
            &relater,
            th(0.9, 0.6),
            &BTreeMap::new(),
            ParallelRelateOptions {
                temporal: &temporal,
                budget: Duration::MAX,
                concurrency: 4,
                candidate_pair_budget: DEFAULT_CANDIDATE_PAIR_BUDGET,
                candidate_cursor: CandidateCursor::start(),
            },
        )
        .expect("relate");

        assert_eq!(relater.count(), 0);
        let partial = partial(&out).expect("unresolved pair is partial");
        assert!(partial.judgments.is_empty());
        assert_eq!(partial.unresolved.len(), 1);
        assert_eq!(partial.unresolved[0].failure.class, expected);
    }
}

#[test]
fn journal_authority_keeps_its_original_pair_direction() {
    let claims = vec![
        claim("claim_aaaaaaaaaaaa", "storage", "storage alpha", 1),
        claim("claim_bbbbbbbbbbbb", "storage", "storage beta", 2),
    ];
    let embedder = FixedEmbedder::new(vec![("storage", vec![1.0, 0.0])], 2);
    let relater = CountingRelater::new();
    let left = SourceSnapshotId::derive("left");
    let right = SourceSnapshotId::derive("right");
    let mut temporal = RelateTemporalPolicy::default();
    temporal.bind_claim(&claims[0].0, &left);
    temporal.bind_claim(&claims[1].0, &right);
    temporal.insert_relation(&left, &right, TemporalRelation::After);
    let mut settled = BTreeMap::new();
    settled.insert(
        (claims[0].0.clone(), claims[1].0.clone()),
        RelationVerdict {
            relation: ClaimRelation::Unrelated,
            score: 1.0,
        },
    );

    let out = relate_claims_settled_parallel_temporal(
        &claims,
        &embedder,
        &relater,
        th(0.9, 0.6),
        &settled,
        ParallelRelateOptions {
            temporal: &temporal,
            budget: Duration::MAX,
            concurrency: 4,
            candidate_pair_budget: DEFAULT_CANDIDATE_PAIR_BUDGET,
            candidate_cursor: CandidateCursor::start(),
        },
    )
    .expect("relate");

    assert_eq!(relater.count(), 0);
    assert_eq!(out.judgments()[0].older_claim, claims[0].0);
    assert_eq!(out.judgments()[0].newer_claim, claims[1].0);
    assert!(out.judgments()[0].reused_authority);
}

#[test]
fn hard_candidate_budget_is_partial_bounded_and_resumable() {
    let claims = (0_u64..5)
        .map(|idx| {
            claim(
                &format!("claim_{idx:012x}"),
                "shared",
                &format!("shared candidate {idx}"),
                idx + 1,
            )
        })
        .collect::<Vec<_>>();
    let embedder = FixedEmbedder::new(vec![("shared", vec![1.0, 0.0])], 2);
    let mut settled = BTreeMap::new();
    let mut passes = 0_usize;
    let mut cursor = CandidateCursor::start();
    let mut observed_cursors = Vec::new();
    loop {
        let relater = CountingRelater::new();
        let out = relate_claims_settled_parallel_temporal(
            &claims,
            &embedder,
            &relater,
            th(0.9, 0.6),
            &settled,
            ParallelRelateOptions {
                temporal: &RelateTemporalPolicy::default(),
                budget: Duration::MAX,
                concurrency: 2,
                candidate_pair_budget: 2,
                candidate_cursor: cursor,
            },
        )
        .expect("bounded relate");
        assert!(relater.count() <= 2, "one pass cannot exceed the hard cap");
        for judgment in out.judgments() {
            settled.insert(
                (judgment.older_claim.clone(), judgment.newer_claim.clone()),
                judgment.verdict,
            );
        }
        passes += 1;
        match &out {
            RelateOutcome::Complete(complete) => {
                assert_eq!(complete.candidate_pairs, 10);
                assert_eq!(settled.len(), 10, "all pairs settle after resumes");
                break;
            }
            RelateOutcome::Partial(partial) => {
                assert!(partial.candidate_pairs <= 2);
                assert_eq!(partial.candidate_pair_budget, 2);
                cursor = partial.next_candidate_cursor;
                observed_cursors.push(cursor.offset());
                assert!(passes < 10, "bounded passes must make progress");
            }
        }
    }
    assert!(passes > 1);
    assert_eq!(observed_cursors, vec![2, 4, 6, 8]);
}

#[test]
fn resumed_completion_matches_one_shot_authority_edges() {
    let claims = (0_u64..5)
        .map(|idx| {
            claim(
                &format!("claim_{idx:012x}"),
                "shared",
                &format!("shared candidate {idx}"),
                idx + 1,
            )
        })
        .collect::<Vec<_>>();
    let embedder = FixedEmbedder::new(vec![("shared", vec![1.0, 0.0])], 2);
    let relater = ScriptedRelater::new(vec![
        ("candidate 0", "candidate 2", ClaimRelation::Supersedes),
        ("candidate 3", "candidate 4", ClaimRelation::Conflict),
    ]);
    let one_shot =
        relate_claims(&claims, &embedder, &relater, th(0.9, 0.6)).expect("one-shot relate");
    let one_shot = complete(&one_shot).expect("one-shot completion");

    let mut settled = BTreeMap::new();
    let mut cursor = CandidateCursor::start();
    let paged = loop {
        let outcome = relate_claims_settled_parallel_temporal(
            &claims,
            &embedder,
            &relater,
            th(0.9, 0.6),
            &settled,
            ParallelRelateOptions {
                temporal: &RelateTemporalPolicy::default(),
                budget: Duration::MAX,
                concurrency: 1,
                candidate_pair_budget: 2,
                candidate_cursor: cursor,
            },
        )
        .expect("paged relate");
        match outcome {
            RelateOutcome::Complete(complete) => break complete,
            RelateOutcome::Partial(partial) => {
                for judgment in partial.judgments {
                    settled.insert(
                        (judgment.older_claim, judgment.newer_claim),
                        judgment.verdict,
                    );
                }
                cursor = partial.next_candidate_cursor;
            }
        }
    };

    assert_eq!(paged.related.supersessions, one_shot.related.supersessions);
    assert_eq!(
        format!("{:?}", paged.related.conflicts),
        format!("{:?}", one_shot.related.conflicts)
    );
    let verdicts = |judgments: &[PairJudgment]| {
        judgments
            .iter()
            .map(|judgment| {
                (
                    judgment.older_claim.clone(),
                    judgment.newer_claim.clone(),
                    judgment.verdict,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        verdicts(&paged.related.judgments),
        verdicts(&one_shot.related.judgments)
    );
}

#[test]
fn changed_model_fingerprint_cannot_rejudge_journal_authority() {
    let claims = vec![
        claim("claim_aaaaaaaaaaaa", "shared", "shared alpha", 1),
        claim("claim_bbbbbbbbbbbb", "shared", "shared beta", 2),
    ];
    let embedder = FixedEmbedder::new(vec![("shared", vec![1.0, 0.0])], 2);
    let mut settled = BTreeMap::new();
    settled.insert(
        (claims[0].0.clone(), claims[1].0.clone()),
        RelationVerdict {
            relation: ClaimRelation::Conflict,
            score: 0.8,
        },
    );
    for fingerprint in ["model-a:prompt-1", "model-b:prompt-99"] {
        let relater = FingerprintRelater {
            inner: CountingRelater::new(),
            fingerprint,
        };
        let out = relate_claims_with_settled(
            &claims,
            &embedder,
            &relater,
            th(0.9, 0.6),
            &settled,
            Duration::MAX,
        )
        .expect("journal authority");
        assert_eq!(relater.inner.count(), 0);
        assert!(out.judgments()[0].reused_authority);
        assert_eq!(
            out.judgments()[0].verdict,
            settled[&(claims[0].0.clone(), claims[1].0.clone())]
        );
    }
}
