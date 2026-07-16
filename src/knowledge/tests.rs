use super::*;

fn descriptor() -> SnapshotDescriptor {
    SnapshotDescriptor {
        workspace_id: WorkspaceId::new("demo").expect("workspace"),
        journal_id: JournalId::new("canonical").expect("journal"),
        frontier: 42,
        anchor_event_id_hex: "ab".repeat(16),
        source_snapshot_id: Some(SourceSnapshotId::derive("source-state")),
    }
}

#[test]
fn snapshot_token_is_deterministic_and_sensitive_to_every_coordinate() {
    let first = descriptor();
    let mut changed = first.clone();
    changed.frontier += 1;
    assert_eq!(
        SnapshotToken::for_descriptor(&first),
        SnapshotToken::for_descriptor(&first)
    );
    assert_ne!(
        SnapshotToken::for_descriptor(&first),
        SnapshotToken::for_descriptor(&changed)
    );
    let token = SnapshotToken::for_descriptor(&first);
    assert_eq!(
        SnapshotToken::resolve_for_journal(token.as_str(), &first.workspace_id, &first.journal_id,),
        Ok(first)
    );
}

#[test]
fn snapshot_token_rejects_tampering_and_cross_workspace_reuse() {
    let descriptor = descriptor();
    let token = SnapshotToken::for_descriptor(&descriptor);
    let changed = token.as_str().replacen(".42.", ".41.", 1);
    assert!(SnapshotToken::resolve_for_journal(
        &changed,
        &descriptor.workspace_id,
        &descriptor.journal_id,
    )
    .is_err());
    assert!(SnapshotToken::resolve_for_journal(
        token.as_str(),
        &WorkspaceId::new("other").expect("workspace"),
        &descriptor.journal_id,
    )
    .is_err());
    assert!(SnapshotToken::resolve_for_journal(
        token.as_str(),
        &descriptor.workspace_id,
        &JournalId::new("replica").expect("journal"),
    )
    .is_err());
}

#[test]
fn git_object_ids_enforce_algorithm_length_and_lowercase_hex() {
    assert!(GitObjectId::new(GitObjectFormat::Sha1, "a".repeat(40)).is_ok());
    assert!(GitObjectId::new(GitObjectFormat::Sha256, "b".repeat(64)).is_ok());
    assert!(GitObjectId::new(GitObjectFormat::Sha1, "A".repeat(40)).is_err());
    assert!(GitObjectId::new(GitObjectFormat::Sha256, "c".repeat(40)).is_err());
}

#[test]
fn evidence_bounds_fail_closed() {
    let occurrence = EvidenceOccurrence {
        occurrence_id: EvidenceOccurrenceId::derive("occurrence"),
        snapshot_id: SourceSnapshotId::derive("snapshot"),
        source_kind: EvidenceSourceKind::Markdown,
        path: "docs/a.md".to_string(),
        byte_range: ByteRange::new(0, 3).expect("range"),
        line_range: LineRange::new(1, 1).expect("line range"),
        git_blob: None,
        source_digest_hex: "d".repeat(64),
        excerpt: "abc".to_string(),
        analyzer_fingerprint: "markdown:v1".to_string(),
        analysis_quality: AnalysisQuality::Syntactic,
    };
    assert_eq!(occurrence.validate(), Ok(()));

    let mut too_large = occurrence;
    too_large.excerpt = "x".repeat(MAX_EVIDENCE_EXCERPT_BYTES + 1);
    assert!(matches!(
        too_large.validate(),
        Err(KnowledgeContractError::ExcerptTooLarge { .. })
    ));
}

#[test]
fn temporal_relation_does_not_collapse_concurrency_into_order() {
    let encoded = serde_json::to_string(&TemporalRelation::Concurrent).expect("serialize");
    assert_eq!(encoded, "\"concurrent\"");
    assert_ne!(TemporalRelation::Concurrent, TemporalRelation::Before);
    assert_ne!(TemporalRelation::Concurrent, TemporalRelation::After);
}
