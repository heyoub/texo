use super::*;

#[test]
fn deploy_schedule_groups_three_days_and_noise_together() {
    let claims = vec![
        claim("claim_aaaaaaaaaaaa", "x", "Deploys happen on Friday", 1),
        claim("claim_bbbbbbbbbbbb", "x", "Deploys moved to Wednesday", 2),
        claim("claim_cccccccccccc", "x", "Deploys moved to Tuesday", 3),
        claim(
            "claim_dddddddddddd",
            "x",
            "dave asked about the deploy day",
            2,
        ),
    ];
    let groups = group_claims(&claims, &deploy_embedder(), 0.9).expect("group");
    assert_eq!(groups.len(), 1, "all four cluster on deploy-day meaning");
    assert_eq!(groups[0].len(), 4);
}

/// Embedder for the release scenario: the two release-schedule claims cluster
/// together, but "Bob owns release approval" is a DIFFERENT subject and must
/// land in its own group (the key dogfood trap — same word, different
/// meaning).
fn release_embedder() -> FixedEmbedder {
    FixedEmbedder::new(
        vec![
            ("releases happen on monday", vec![1.0, 0.0]),
            ("go out on friday", vec![0.95, 0.05]),
            ("bob owns release approval", vec![0.0, 1.0]),
        ],
        2,
    )
}

#[test]
fn release_schedule_splits_from_release_approval_by_meaning() {
    let claims = vec![
        claim("claim_aaaaaaaaaaaa", "x", "Releases happen on Monday", 1),
        claim("claim_bbbbbbbbbbbb", "x", "Releases go out on Friday", 2),
        claim("claim_cccccccccccc", "x", "Bob owns release approval", 3),
    ];
    let groups = group_claims(&claims, &release_embedder(), 0.9).expect("group");
    assert_eq!(
        groups.len(),
        2,
        "schedule and approval are different subjects"
    );
    // The schedule pair groups together; approval is alone.
    let sizes: Vec<usize> = {
        let mut s: Vec<usize> = groups.iter().map(Vec::len).collect();
        s.sort_unstable();
        s
    };
    assert_eq!(sizes, vec![1, 2]);
}

#[test]
fn backend_error_propagates() {
    struct FailingEmbedder;
    impl Embedder for FailingEmbedder {
        fn embed(&self, _text: &str) -> Result<Vec<f32>, SemanticsError> {
            Err(SemanticsError::DimensionMismatch {
                expected: 2,
                actual: 1,
            })
        }
    }
    let claims = vec![claim("claim_aaaaaaaaaaaa", "x", "anything", 1)];
    let err = group_claims(&claims, &FailingEmbedder, 0.9).expect_err("must propagate");
    assert!(matches!(err, PipelineError::Semantics(_)));
}

#[test]
fn group_claims_empty_input_is_empty() {
    let embedder = FixedEmbedder::new(Vec::new(), 2);
    assert!(group_claims(&[], &embedder, 0.9).expect("group").is_empty());
}

#[test]
fn grouping_is_transitive_via_connected_components() {
    // A links B, B links C, but A does not directly link C; connected
    // components still place all three in one group.
    let claims = vec![
        claim("claim_aaaaaaaaaaaa", "x", "alpha", 1),
        claim("claim_bbbbbbbbbbbb", "x", "bravo", 2),
        claim("claim_cccccccccccc", "x", "charlie", 3),
    ];
    let embedder = FixedEmbedder::new(
        vec![
            ("alpha", vec![1.0, 0.0]),
            ("bravo", vec![0.95, 0.31]),
            ("charlie", vec![0.80, 0.60]),
        ],
        2,
    );
    // alpha-bravo cosine ~0.95 (>=0.9), bravo-charlie ~0.95 (>=0.9), but
    // alpha-charlie ~0.80 (<0.9): only connected components unite all three.
    let groups = group_claims(&claims, &embedder, 0.9).expect("group");
    assert_eq!(groups.len(), 1, "transitive chain forms one component");
    assert_eq!(groups[0].len(), 3);
}

// --- relate_claims (the LLM-relation-judge path) ---

#[test]
fn relate_supersession_chain_picks_newest_winner_and_ignores_noise() {
    let claims = vec![
        claim("claim_aaaaaaaaaaaa", "x", "Deploys happen on Friday", 1),
        claim("claim_bbbbbbbbbbbb", "x", "Deploys moved to Wednesday", 2),
        claim("claim_cccccccccccc", "x", "Deploys moved to Tuesday", 3),
        claim(
            "claim_dddddddddddd",
            "x",
            "dave asked about the deploy day",
            2,
        ),
    ];
    // The judge reports each newer deploy decision as superseding the older;
    // the noise question is unrelated to every deploy claim.
    let relater = ScriptedRelater::new(vec![
        ("friday", "wednesday", ClaimRelation::Supersedes),
        ("friday", "tuesday", ClaimRelation::Supersedes),
        ("wednesday", "tuesday", ClaimRelation::Supersedes),
    ]);
    let out = relate_claims(&claims, &deploy_embedder(), &relater, th(0.9, 0.9)).expect("relate");
    let complete_related = related(&out).expect("complete");

    // Friday and Wednesday each superseded by Tuesday (the newest winner).
    assert_eq!(complete_related.supersessions.len(), 2);
    let pairs: Vec<(&str, &str)> = complete_related
        .supersessions
        .iter()
        .map(|(o, n, _)| (o.as_str(), n.as_str()))
        .collect();
    assert!(pairs.contains(&("claim_aaaaaaaaaaaa", "claim_cccccccccccc")));
    assert!(pairs.contains(&("claim_bbbbbbbbbbbb", "claim_cccccccccccc")));
    assert!(
        !pairs
            .iter()
            .any(|(o, n)| *o == "claim_dddddddddddd" || *n == "claim_dddddddddddd"),
        "noise never participates in supersession"
    );
    assert!(
        complete_related.conflicts.is_empty(),
        "no conflicts in a clean chain"
    );
}

#[test]
fn relate_release_disagreement_is_conflict_not_supersession() {
    let claims = vec![
        claim("claim_aaaaaaaaaaaa", "x", "Releases happen on Monday", 1),
        claim("claim_bbbbbbbbbbbb", "x", "Releases go out on Friday", 2),
        claim("claim_cccccccccccc", "x", "Bob owns release approval", 3),
    ];
    // Monday vs Friday disagree with no update intent -> conflict. Approval is
    // a different subject and never grouped with the schedule pair.
    let relater = ScriptedRelater::new(vec![("monday", "friday", ClaimRelation::Conflict)]);
    let out = relate_claims(&claims, &release_embedder(), &relater, th(0.9, 0.9)).expect("relate");

    assert!(
        related(&out).expect("complete").supersessions.is_empty(),
        "a flat disagreement is not a supersession"
    );
    assert_eq!(
        related(&out).expect("complete").conflicts.len(),
        1,
        "exactly one release conflict"
    );
    let entry = &related(&out).expect("complete").conflicts[0];
    let mut pair = [entry.claim_a.as_str(), entry.claim_b.as_str()];
    pair.sort_unstable();
    assert_eq!(pair, ["claim_aaaaaaaaaaaa", "claim_bbbbbbbbbbbb"]);
    assert_eq!(entry.status, ConflictStatus::Open);
}

#[test]
fn relate_superseded_claim_cannot_also_conflict() {
    // A claim that is superseded must not surface as a live conflict, even if
    // the judge also reports a contradicting peer.
    let claims = vec![
        claim("claim_aaaaaaaaaaaa", "x", "Deploys happen on Friday", 1),
        claim("claim_bbbbbbbbbbbb", "x", "Deploys moved to Tuesday", 3),
        claim("claim_cccccccccccc", "x", "Deploys happen on Monday", 2),
    ];
    let relater = ScriptedRelater::new(vec![
        ("friday", "tuesday", ClaimRelation::Supersedes),
        ("monday", "tuesday", ClaimRelation::Supersedes),
        ("friday", "monday", ClaimRelation::Conflict),
    ]);
    // All three deploy-day claims must cluster (deploy_embedder omits Monday).
    let embedder = FixedEmbedder::new(
        vec![
            ("friday", vec![1.0, 0.0, 0.0]),
            ("tuesday", vec![0.97, 0.12, 0.0]),
            ("monday", vec![0.96, 0.14, 0.0]),
        ],
        3,
    );
    let out = relate_claims(&claims, &embedder, &relater, th(0.9, 0.9)).expect("relate");
    // Friday and Monday are both superseded by Tuesday, so the Friday/Monday
    // conflict is dropped.
    assert_eq!(related(&out).expect("complete").supersessions.len(), 2);
    assert!(
        related(&out).expect("complete").conflicts.is_empty(),
        "conflict involving a superseded claim is dropped"
    );
}

#[test]
fn relate_duplicate_text_is_skipped() {
    let claims = vec![
        claim("claim_aaaaaaaaaaaa", "x", "Deploys moved to Tuesday", 1),
        claim("claim_bbbbbbbbbbbb", "x", "Deploys moved to Tuesday", 2),
    ];
    let embedder = FixedEmbedder::new(vec![("tuesday", vec![1.0, 0.0])], 2);
    // Even if the judge would fire, identical normalized text is never judged.
    let relater = ScriptedRelater::new(vec![("tuesday", "tuesday", ClaimRelation::Supersedes)]);
    let out = relate_claims(&claims, &embedder, &relater, th(0.9, 0.9)).expect("relate");
    assert!(related(&out).expect("complete").supersessions.is_empty());
    assert!(related(&out).expect("complete").conflicts.is_empty());
}

/// Call counter proving a candidate-generation gate drops a pair before the
/// judge boundary.
#[derive(Default)]
struct NeverRelater(std::cell::Cell<usize>);
impl ClaimRelater for NeverRelater {
    fn relate(&self, _older: &str, _newer: &str) -> Result<RelationVerdict, SemanticsError> {
        self.0.set(self.0.get().saturating_add(1));
        Ok(RelationVerdict {
            relation: ClaimRelation::Unrelated,
            score: 1.0,
        })
    }
    fn fingerprint(&self) -> String {
        "never".to_owned()
    }
}

#[test]
fn relate_prefilter_skips_low_similarity_pairs_within_a_cluster() {
    let claims = vec![
        claim("claim_aaaaaaaaaaaa", "x", "alpha subject", 1),
        claim("claim_bbbbbbbbbbbb", "x", "beta subject", 2),
    ];
    // Cosine ~0.30: above the cluster link threshold (0.2), so both claims
    // share one cluster — but below the prefilter (0.5), so the pair must
    // still be gated out before any judge call.
    let embedder = FixedEmbedder::new(
        vec![("alpha", vec![1.0, 0.0]), ("beta", vec![0.3, 0.954])],
        2,
    );
    let relater = NeverRelater::default();
    let out = relate_claims(&claims, &embedder, &relater, th(0.2, 0.5)).expect("relate");
    assert_eq!(relater.0.get(), 0);
    assert!(related(&out).expect("complete").supersessions.is_empty());
    assert!(related(&out).expect("complete").conflicts.is_empty());
}

#[test]
fn paged_relate_keeps_recall_below_the_old_cluster_cutoff() {
    // Cosine ~0.87 clears the lower semantic floor even though it falls below
    // the former connected-component threshold. Bounded paging controls cost
    // without dropping the pair from semantic consideration.
    let claims = vec![
        claim("claim_aaaaaaaaaaaa", "x", "alpha subject", 1),
        claim("claim_bbbbbbbbbbbb", "x", "beta subject", 2),
    ];
    let embedder = FixedEmbedder::new(
        vec![("alpha", vec![1.0, 0.0]), ("beta", vec![0.87, 0.493])],
        2,
    );
    let relater = NeverRelater::default();
    let out = relate_claims(&claims, &embedder, &relater, th(0.95, 0.6)).expect("relate");
    assert_eq!(relater.0.get(), 1);
    assert!(related(&out).expect("complete").supersessions.is_empty());
    assert!(related(&out).expect("complete").conflicts.is_empty());
}

#[test]
fn relate_empty_and_singleton_inputs_are_inert() {
    let embedder = FixedEmbedder::new(Vec::new(), 2);
    let relater = ScriptedRelater::new(Vec::new());
    let empty = relate_claims(&[], &embedder, &relater, th(0.5, 0.5)).expect("relate empty");
    assert!(
        related(&empty).expect("complete").supersessions.is_empty()
            && related(&empty).expect("complete").conflicts.is_empty()
    );

    let one = vec![claim("claim_aaaaaaaaaaaa", "x", "Deploys on Tuesday", 1)];
    let single = relate_claims(&one, &embedder, &relater, th(0.5, 0.5)).expect("relate one");
    assert!(
        related(&single).expect("complete").supersessions.is_empty()
            && related(&single).expect("complete").conflicts.is_empty()
    );
}

#[test]
fn relate_propagates_embedder_failure() {
    struct FailingEmbedder;
    impl Embedder for FailingEmbedder {
        fn embed(&self, _text: &str) -> Result<Vec<f32>, SemanticsError> {
            Err(SemanticsError::DimensionMismatch {
                expected: 2,
                actual: 1,
            })
        }
    }
    let claims = vec![
        claim("claim_aaaaaaaaaaaa", "x", "one", 1),
        claim("claim_bbbbbbbbbbbb", "x", "two", 2),
    ];
    let relater = ScriptedRelater::new(Vec::new());
    let err =
        relate_claims(&claims, &FailingEmbedder, &relater, th(0.5, 0.5)).expect_err("propagate");
    assert!(matches!(err, PipelineError::Semantics(_)));
}
