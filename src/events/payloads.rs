//! Versioned texo event payloads.

use batpak::event::{EventKind, EventPayload, Upcast, UpcastError};
use rmpv::Value;
use serde::{Deserialize, Serialize};

use crate::events::ids::{ClaimId, WorkspaceId};
use crate::events::machines::{
    transition_id, TransitionCauseV1, TransitionRecordV1, CLAIM_MACHINE, CONFLICT_MACHINE,
};
use crate::knowledge::{
    CodeIndexFormat, CodeIndexId, EvidenceLinkMethod, EvidenceOccurrence, EvidenceOccurrenceId,
    EvidenceStance, GitObjectId, KnowledgeCoverage, RepositoryId, SourceSnapshotId,
    TemporalRelation,
};
use crate::relate::settlement::{RelationFailureClass, SettledRelation};

/// A source document observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 1, version = 2)]
pub struct SourceObservedV2 {
    /// Stable source identifier.
    pub source_id: String,
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// Source kind label.
    pub source_kind: String,
    /// Source path relative to the workspace root.
    pub path: String,
    /// BLAKE3 body hash as lowercase hex.
    pub body_hash_hex: String,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
}

/// A claim extracted from a source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 2, version = 2)]
pub struct ClaimRecordedV2 {
    /// Stable claim identifier.
    pub claim_id: String,
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// Stable source identifier.
    pub source_id: String,
    /// Source path relative to the workspace root.
    pub source_path: String,
    /// One-based starting line.
    pub line_start: u32,
    /// One-based ending line.
    pub line_end: u32,
    /// Zero-based starting character offset.
    pub char_start: u32,
    /// Zero-based ending character offset.
    pub char_end: u32,
    /// Extracted claim text.
    pub text: String,
    /// Normalized claim text.
    pub normalized_text: String,
    /// Optional subject hint captured by extraction.
    pub subject_hint: Option<String>,
    /// Optional predicate hint captured by extraction.
    pub predicate_hint: Option<String>,
    /// Optional object hint captured by extraction.
    pub object_hint: Option<String>,
    /// Extractor confidence in parts per million.
    pub confidence_ppm: u32,
    /// Extractor implementation kind.
    pub extractor_kind: String,
    /// Extractor model identifier.
    pub extractor_model: String,
    /// Prompt version identifier.
    pub prompt_version: String,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
}

/// A claim supersession decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 3, version = 2)]
pub struct ClaimSupersededV2 {
    /// Superseded claim identifier.
    pub old_claim_id: String,
    /// Replacement claim identifier.
    pub new_claim_id: String,
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// Human-readable supersession reason.
    pub reason: String,
    /// Actor that made the decision.
    pub decided_by: String,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
    /// State-machine transition receipt.
    pub transition: TransitionRecordV1,
}

/// An opened conflict between two claims.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 4, version = 2)]
pub struct ConflictOpenedV2 {
    /// Stable conflict identifier.
    pub conflict_id: String,
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// First conflicting claim identifier.
    pub claim_a: String,
    /// Second conflicting claim identifier.
    pub claim_b: String,
    /// Human-readable conflict reason.
    pub reason: String,
    /// Detector implementation that opened the conflict.
    pub detector: String,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
    /// State-machine transition receipt.
    pub transition: TransitionRecordV1,
}

/// A compiled onboarding artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 5, version = 2)]
pub struct OnboardingCompiledV2 {
    /// Stable compiled document identifier.
    pub doc_id: String,
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// Output path relative to the workspace root.
    pub output_path: String,
    /// Source claim identifiers that contributed to the artifact.
    pub source_claim_ids: Vec<String>,
    /// Journal sequence replayed through for the compile.
    pub replayed_through_sequence: u64,
    /// Compile wall-clock time in milliseconds.
    pub compiled_at_ms: u64,
}

/// A resolved or ignored conflict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 6, version = 1)]
pub struct ConflictResolvedV2 {
    /// Stable conflict identifier.
    pub conflict_id: String,
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// Resolution value, either `resolved` or `ignored`.
    pub resolution: String,
    /// Actor that resolved or ignored the conflict.
    pub resolved_by: String,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
    /// State-machine transition receipt.
    pub transition: TransitionRecordV1,
}

/// Workspace initialization marker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 7, version = 1)]
pub struct WorkspaceInitializedV2 {
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// Schema identifier, expected to be `texo.v2`.
    pub schema: String,
    /// Configuration digest as lowercase hex.
    pub config_digest_hex: String,
    /// Creation wall-clock time in milliseconds.
    pub created_at_ms: u64,
}

/// One turn in a session transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 8, version = 1)]
pub struct SessionTurnV1 {
    /// Stable session identifier.
    pub session_id: String,
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// Speaker label, either `user` or `assistant`.
    pub speaker: String,
    /// Turn text.
    pub text: String,
    /// Monotonic turn number within the session.
    pub turn_no: u32,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
}

/// A completed semantic judgment for one logical relation pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 9, version = 1)]
pub struct RelationJudgedV1 {
    /// Workspace scope identifier.
    pub workspace_id: WorkspaceId,
    /// Older claim in commit-order semantics.
    pub older_claim: ClaimId,
    /// Newer claim in commit-order semantics.
    pub newer_claim: ClaimId,
    /// Closed semantic verdict.
    pub relation: SettledRelation,
    /// Confidence score in parts per million.
    pub score_ppm: u32,
    /// Provider/model/prompt attempt fingerprint.
    pub judge_fingerprint: String,
    /// Paid-verdict cache key as lowercase hex.
    pub cache_key_hex: String,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
}

/// A failed semantic attempt for one logical relation pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 10, version = 1)]
pub struct RelationDeferredV1 {
    /// Workspace scope identifier.
    pub workspace_id: WorkspaceId,
    /// Older claim.
    pub older_claim: ClaimId,
    /// Newer claim.
    pub newer_claim: ClaimId,
    /// Closed failure class.
    pub failure_class: RelationFailureClass,
    /// Provider attempts performed for this deferral.
    pub attempts: u32,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
}

/// One frozen Git commit plus index/worktree overlay observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 11, version = 1)]
pub struct SourceSnapshotRecordedV1 {
    /// Workspace scope identifier.
    pub workspace_id: WorkspaceId,
    /// Stable local repository identity.
    pub repository_id: RepositoryId,
    /// Content-addressed snapshot identity.
    pub snapshot_id: SourceSnapshotId,
    /// Commit resolved once at capture start.
    pub base_commit: GitObjectId,
    /// Tree referenced by the base commit.
    pub base_tree: GitObjectId,
    /// Digest of the exact Git index bytes.
    pub index_digest_hex: String,
    /// Digest of the sorted frozen worktree overlay.
    pub overlay_digest_hex: String,
    /// Whether the frozen overlay differs from the base tree.
    pub dirty: bool,
    /// Bounded capture coverage and omissions.
    pub coverage: KnowledgeCoverage,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
}

/// One exact bounded occurrence of source evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 12, version = 1)]
pub struct EvidenceOccurrenceRecordedV1 {
    /// Workspace scope identifier.
    pub workspace_id: WorkspaceId,
    /// Exact occurrence and analyzer provenance.
    pub occurrence: EvidenceOccurrence,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
}

/// Durable link from an assertion to exact evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 13, version = 1)]
pub struct ClaimEvidenceLinkedV1 {
    /// Workspace scope identifier.
    pub workspace_id: WorkspaceId,
    /// Existing semantic claim identity.
    pub claim_id: ClaimId,
    /// Exact evidence occurrence identity.
    pub occurrence_id: EvidenceOccurrenceId,
    /// How the evidence bears on the claim.
    pub stance: EvidenceStance,
    /// Mechanism whose result policy accepted.
    pub method: EvidenceLinkMethod,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
}

/// Registration of one disposable, content-addressed code index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 14, version = 1)]
pub struct CodeIndexRecordedV1 {
    /// Workspace scope identifier.
    pub workspace_id: WorkspaceId,
    /// Frozen source snapshot indexed.
    pub snapshot_id: SourceSnapshotId,
    /// Content-addressed code-index identity.
    pub index_id: CodeIndexId,
    /// Artifact encoding.
    pub format: CodeIndexFormat,
    /// Analyzer implementation and version.
    pub analyzer_fingerprint: String,
    /// BLAKE3 digest of the disposable artifact bytes.
    pub artifact_digest_hex: String,
    /// Bounded index coverage and omissions.
    pub coverage: KnowledgeCoverage,
    /// Observation wall-clock time in milliseconds.
    pub observed_at_ms: u64,
}

/// One replayable Git-ancestry comparison between frozen source snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 15, version = 1)]
pub struct SourceSnapshotRelationV1 {
    /// Workspace scope identifier.
    pub workspace_id: WorkspaceId,
    /// Repository whose object DAG supplied the comparison.
    pub repository_id: RepositoryId,
    /// Previously recorded snapshot.
    pub left_snapshot_id: SourceSnapshotId,
    /// Newly recorded snapshot.
    pub right_snapshot_id: SourceSnapshotId,
    /// Commit underlying the left snapshot.
    pub left_commit: GitObjectId,
    /// Commit underlying the right snapshot.
    pub right_commit: GitObjectId,
    /// Ancestry relation from left to right.
    pub relation: TemporalRelation,
    /// Observation wall-clock time in milliseconds; never an ordering input.
    pub observed_at_ms: u64,
}

struct SourceObservedV1ToV2;
struct ClaimRecordedV1ToV2;
struct ClaimSupersededV1ToV2;
struct ConflictOpenedV1ToV2;
struct OnboardingCompiledV1ToV2;

impl Upcast for SourceObservedV1ToV2 {
    const KIND: EventKind = <SourceObservedV2 as EventPayload>::KIND;
    const FROM_VERSION: u16 = 1;

    fn upcast(value: Value) -> Result<Value, UpcastError> {
        Ok(value)
    }
}

impl Upcast for ClaimRecordedV1ToV2 {
    const KIND: EventKind = <ClaimRecordedV2 as EventPayload>::KIND;
    const FROM_VERSION: u16 = 1;

    fn upcast(mut value: Value) -> Result<Value, UpcastError> {
        let fields = map_fields_mut(&mut value, "ClaimRecordedV2")?;
        insert_if_missing(fields, "char_start", Value::from(0_u32));
        insert_if_missing(fields, "char_end", Value::from(0_u32));
        insert_if_missing(fields, "extractor_kind", Value::from("unknown"));
        insert_if_missing(fields, "extractor_model", Value::from("unknown"));
        insert_if_missing(fields, "prompt_version", Value::from("unknown"));
        Ok(value)
    }
}

impl Upcast for ClaimSupersededV1ToV2 {
    const KIND: EventKind = <ClaimSupersededV2 as EventPayload>::KIND;
    const FROM_VERSION: u16 = 1;

    fn upcast(mut value: Value) -> Result<Value, UpcastError> {
        let fields = map_fields_mut(&mut value, "ClaimSupersededV2")?;
        let old_claim_id = string_field(fields, "old_claim_id")?;
        let observed_at_ms = u64_field(fields, "observed_at_ms")?;
        insert_if_missing(
            fields,
            "transition",
            transition_value(CLAIM_MACHINE, &old_claim_id, 1, 2, observed_at_ms),
        );
        Ok(value)
    }
}

impl Upcast for ConflictOpenedV1ToV2 {
    const KIND: EventKind = <ConflictOpenedV2 as EventPayload>::KIND;
    const FROM_VERSION: u16 = 1;

    fn upcast(mut value: Value) -> Result<Value, UpcastError> {
        let fields = map_fields_mut(&mut value, "ConflictOpenedV2")?;
        let conflict_id = string_field(fields, "conflict_id")?;
        let observed_at_ms = u64_field(fields, "observed_at_ms")?;
        insert_if_missing(fields, "detector", Value::from("unknown"));
        insert_if_missing(
            fields,
            "transition",
            transition_value(CONFLICT_MACHINE, &conflict_id, 0, 1, observed_at_ms),
        );
        Ok(value)
    }
}

impl Upcast for OnboardingCompiledV1ToV2 {
    const KIND: EventKind = <OnboardingCompiledV2 as EventPayload>::KIND;
    const FROM_VERSION: u16 = 1;

    fn upcast(value: Value) -> Result<Value, UpcastError> {
        Ok(value)
    }
}

batpak::register_upcast!(SourceObservedV1ToV2);
batpak::register_upcast!(ClaimRecordedV1ToV2);
batpak::register_upcast!(ClaimSupersededV1ToV2);
batpak::register_upcast!(ConflictOpenedV1ToV2);
batpak::register_upcast!(OnboardingCompiledV1ToV2);

fn map_fields_mut<'a>(
    value: &'a mut Value,
    payload_name: &str,
) -> Result<&'a mut Vec<(Value, Value)>, UpcastError> {
    match value {
        Value::Map(fields) => Ok(fields),
        Value::Nil
        | Value::Boolean(_)
        | Value::Integer(_)
        | Value::F32(_)
        | Value::F64(_)
        | Value::String(_)
        | Value::Binary(_)
        | Value::Array(_)
        | Value::Ext(_, _) => Err(UpcastError::ValueCodec(format!(
            "{payload_name} v1 upcast expected msgpack map"
        ))),
    }
}

fn insert_if_missing(fields: &mut Vec<(Value, Value)>, key: &str, value: Value) {
    if fields.iter().any(|(field, _)| field.as_str() == Some(key)) {
        return;
    }
    fields.push((Value::from(key), value));
}

fn string_field(fields: &[(Value, Value)], key: &str) -> Result<String, UpcastError> {
    fields
        .iter()
        .find(|(field, _)| field.as_str() == Some(key))
        .and_then(|(_, value)| value.as_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| UpcastError::ValueCodec(format!("missing string field `{key}`")))
}

fn u64_field(fields: &[(Value, Value)], key: &str) -> Result<u64, UpcastError> {
    fields
        .iter()
        .find(|(field, _)| field.as_str() == Some(key))
        .and_then(|(_, value)| value.as_u64())
        .ok_or_else(|| UpcastError::ValueCodec(format!("missing u64 field `{key}`")))
}

fn transition_value(
    machine: &str,
    entity: &str,
    previous_state: u64,
    next_state: u64,
    observed_at_ms: u64,
) -> Value {
    let causes: Vec<TransitionCauseV1> = Vec::new();
    Value::Map(vec![
        (Value::from("schema_version"), Value::from(1_u32)),
        (Value::from("machine"), Value::from(machine)),
        (Value::from("previous_state"), Value::from(previous_state)),
        (Value::from("next_state"), Value::from(next_state)),
        (
            Value::from("transition_id_hex"),
            Value::from(transition_id(
                machine,
                entity,
                previous_state,
                next_state,
                &causes,
                observed_at_ms,
            )),
        ),
        (Value::from("causes"), Value::Array(Vec::new())),
    ])
}

#[cfg(test)]
mod tests {
    use serde::de::DeserializeOwned;

    use super::*;
    use crate::events::machines::{transition_record, CLAIM_MACHINE, CONFLICT_MACHINE};
    use crate::knowledge::{
        AnalysisQuality, ByteRange, CoverageGap, CoverageGapKind, EvidenceSourceKind,
        GitObjectFormat, LineRange,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn assert_round_trip<T>(value: &T) -> TestResult
    where
        T: std::fmt::Debug + PartialEq + Serialize + DeserializeOwned,
    {
        let json = serde_json::to_string(value)?;
        let parsed = serde_json::from_str::<T>(&json)?;
        assert_eq!(&parsed, value);
        Ok(())
    }

    fn claim_transition() -> TransitionRecordV1 {
        transition_record(CLAIM_MACHINE, "claim_a", 1, 2, Vec::new(), 100)
    }

    fn conflict_transition(next_state: u64) -> TransitionRecordV1 {
        transition_record(
            CONFLICT_MACHINE,
            "conflict_a",
            1,
            next_state,
            Vec::new(),
            100,
        )
    }

    #[test]
    fn source_observed_round_trips() -> TestResult {
        assert_round_trip(&SourceObservedV2 {
            source_id: "src_a".to_string(),
            workspace_id: "demo".to_string(),
            source_kind: "markdown".to_string(),
            path: "docs/a.md".to_string(),
            body_hash_hex: "abc".to_string(),
            observed_at_ms: 1,
        })
    }

    #[test]
    fn claim_recorded_round_trips() -> TestResult {
        assert_round_trip(&ClaimRecordedV2 {
            claim_id: "claim_a".to_string(),
            workspace_id: "demo".to_string(),
            source_id: "src_a".to_string(),
            source_path: "docs/a.md".to_string(),
            line_start: 1,
            line_end: 2,
            char_start: 3,
            char_end: 4,
            text: "A claim".to_string(),
            normalized_text: "a claim".to_string(),
            subject_hint: Some("a".to_string()),
            predicate_hint: None,
            object_hint: Some("claim".to_string()),
            confidence_ppm: 900_000,
            extractor_kind: "heuristic".to_string(),
            extractor_model: "none".to_string(),
            prompt_version: "v1".to_string(),
            observed_at_ms: 5,
        })
    }

    #[test]
    fn claim_superseded_round_trips() -> TestResult {
        assert_round_trip(&ClaimSupersededV2 {
            old_claim_id: "claim_a".to_string(),
            new_claim_id: "claim_b".to_string(),
            workspace_id: "demo".to_string(),
            reason: "newer".to_string(),
            decided_by: "human".to_string(),
            observed_at_ms: 6,
            transition: claim_transition(),
        })
    }

    #[test]
    fn conflict_opened_round_trips() -> TestResult {
        assert_round_trip(&ConflictOpenedV2 {
            conflict_id: "conflict_a".to_string(),
            workspace_id: "demo".to_string(),
            claim_a: "claim_a".to_string(),
            claim_b: "claim_b".to_string(),
            reason: "contradiction".to_string(),
            detector: "scripted".to_string(),
            observed_at_ms: 7,
            transition: transition_record(CONFLICT_MACHINE, "conflict_a", 0, 1, Vec::new(), 7),
        })
    }

    #[test]
    fn onboarding_compiled_round_trips() -> TestResult {
        assert_round_trip(&OnboardingCompiledV2 {
            doc_id: "doc_a".to_string(),
            workspace_id: "demo".to_string(),
            output_path: "public/index.html".to_string(),
            source_claim_ids: vec!["claim_a".to_string()],
            replayed_through_sequence: 9,
            compiled_at_ms: 10,
        })
    }

    #[test]
    fn conflict_resolved_round_trips() -> TestResult {
        assert_round_trip(&ConflictResolvedV2 {
            conflict_id: "conflict_a".to_string(),
            workspace_id: "demo".to_string(),
            resolution: "resolved".to_string(),
            resolved_by: "human".to_string(),
            observed_at_ms: 11,
            transition: conflict_transition(2),
        })
    }

    #[test]
    fn workspace_initialized_round_trips() -> TestResult {
        assert_round_trip(&WorkspaceInitializedV2 {
            workspace_id: "demo".to_string(),
            schema: "texo.v2".to_string(),
            config_digest_hex: "abc".to_string(),
            created_at_ms: 12,
        })
    }

    #[test]
    fn session_turn_round_trips() -> TestResult {
        assert_round_trip(&SessionTurnV1 {
            session_id: "session_a".to_string(),
            workspace_id: "demo".to_string(),
            speaker: "user".to_string(),
            text: "hello".to_string(),
            turn_no: 1,
            observed_at_ms: 13,
        })
    }

    #[test]
    fn relation_settlement_payloads_round_trip() -> TestResult {
        let workspace_id = WorkspaceId::new("demo")?;
        let older_claim = ClaimId::try_from("claim_aaaaaaaaaaaa")?;
        let newer_claim = ClaimId::try_from("claim_bbbbbbbbbbbb")?;
        assert_round_trip(&RelationJudgedV1 {
            workspace_id: workspace_id.clone(),
            older_claim: older_claim.clone(),
            newer_claim: newer_claim.clone(),
            relation: SettledRelation::Supersedes,
            score_ppm: 900_000,
            judge_fingerprint: "openrouter:model|relation-v2".to_string(),
            cache_key_hex: "abc".to_string(),
            observed_at_ms: 14,
        })?;
        assert_round_trip(&RelationDeferredV1 {
            workspace_id,
            older_claim,
            newer_claim,
            failure_class: RelationFailureClass::Deadline,
            attempts: 5,
            observed_at_ms: 15,
        })
    }

    #[test]
    fn knowledge_payloads_round_trip() -> TestResult {
        let workspace_id = WorkspaceId::new("demo")?;
        let snapshot_id = SourceSnapshotId::derive("snapshot");
        let occurrence_id = EvidenceOccurrenceId::derive("occurrence");
        let coverage = KnowledgeCoverage {
            analysis_quality: AnalysisQuality::Syntactic,
            sources_examined: 1,
            occurrences: 1,
            truncated: false,
            gaps: vec![CoverageGap {
                path: Some("vendor".to_string()),
                kind: CoverageGapKind::Gitlink,
            }],
        };
        assert_round_trip(&SourceSnapshotRecordedV1 {
            workspace_id: workspace_id.clone(),
            repository_id: RepositoryId::derive("repository"),
            snapshot_id: snapshot_id.clone(),
            base_commit: GitObjectId::new(GitObjectFormat::Sha1, "a".repeat(40))?,
            base_tree: GitObjectId::new(GitObjectFormat::Sha1, "b".repeat(40))?,
            index_digest_hex: "c".repeat(64),
            overlay_digest_hex: "d".repeat(64),
            dirty: true,
            coverage: coverage.clone(),
            observed_at_ms: 16,
        })?;
        assert_round_trip(&EvidenceOccurrenceRecordedV1 {
            workspace_id: workspace_id.clone(),
            occurrence: EvidenceOccurrence {
                occurrence_id: occurrence_id.clone(),
                snapshot_id: snapshot_id.clone(),
                source_kind: EvidenceSourceKind::WorktreeOverlay,
                path: "docs/a.md".to_string(),
                byte_range: ByteRange::new(0, 5)?,
                line_range: LineRange::new(1, 1)?,
                git_blob: None,
                source_digest_hex: "e".repeat(64),
                excerpt: "claim".to_string(),
                analyzer_fingerprint: "markdown:heuristic:v1".to_string(),
                analysis_quality: AnalysisQuality::Syntactic,
            },
            observed_at_ms: 17,
        })?;
        assert_round_trip(&ClaimEvidenceLinkedV1 {
            workspace_id: workspace_id.clone(),
            claim_id: ClaimId::try_from("claim_aaaaaaaaaaaa")?,
            occurrence_id,
            stance: EvidenceStance::Supports,
            method: EvidenceLinkMethod::Deterministic,
            observed_at_ms: 18,
        })?;
        assert_round_trip(&SourceSnapshotRelationV1 {
            workspace_id: workspace_id.clone(),
            repository_id: RepositoryId::derive("repository"),
            left_snapshot_id: snapshot_id.clone(),
            right_snapshot_id: SourceSnapshotId::derive("snapshot-next"),
            left_commit: GitObjectId::new(GitObjectFormat::Sha1, "a".repeat(40))?,
            right_commit: GitObjectId::new(GitObjectFormat::Sha1, "b".repeat(40))?,
            relation: TemporalRelation::Before,
            observed_at_ms: 19,
        })?;
        assert_round_trip(&CodeIndexRecordedV1 {
            workspace_id,
            snapshot_id,
            index_id: CodeIndexId::derive("index"),
            format: CodeIndexFormat::Syntax,
            analyzer_fingerprint: "tree-sitter-rust:1".to_string(),
            artifact_digest_hex: "f".repeat(64),
            coverage,
            observed_at_ms: 20,
        })
    }
}
