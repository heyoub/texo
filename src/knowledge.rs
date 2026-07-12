//! Frozen contracts for snapshot-consistent evidence and code knowledge.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::events::ids::{blake3_hash_hex, WorkspaceId};

/// Maximum exact evidence excerpt carried by one durable occurrence.
pub const MAX_EVIDENCE_EXCERPT_BYTES: usize = 4 * 1024;

/// Failure to construct a knowledge contract value.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum KnowledgeContractError {
    /// A digest was not lowercase hexadecimal with the required length.
    #[error("{field} must be exactly {length} lowercase hexadecimal characters")]
    InvalidDigest {
        /// Field being validated.
        field: &'static str,
        /// Required character length.
        length: usize,
    },
    /// A half-open byte range was reversed.
    #[error("byte range start {start} exceeds end {end}")]
    ReversedRange {
        /// Inclusive range start.
        start: u64,
        /// Exclusive range end.
        end: u64,
    },
    /// An evidence excerpt exceeded its durable bound.
    #[error("evidence excerpt is {actual} bytes; maximum is {maximum}")]
    ExcerptTooLarge {
        /// Supplied byte length.
        actual: usize,
        /// Maximum byte length.
        maximum: usize,
    },
}

macro_rules! knowledge_id {
    ($name:ident, $prefix:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Derive a content-addressed identifier from canonical material.
            #[must_use]
            pub fn derive(material: &str) -> Self {
                let digest = blake3_hash_hex(material);
                Self(format!(concat!($prefix, "{}"), &digest[..24]))
            }

            /// Borrow the identifier.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

knowledge_id!(
    RepositoryId,
    "repo_",
    "Stable repository identity scoped by Texo configuration."
);
knowledge_id!(
    SourceSnapshotId,
    "snapshot_",
    "Identity of one frozen source snapshot."
);
knowledge_id!(
    EvidenceOccurrenceId,
    "evidence_",
    "Identity of one exact occurrence of evidence."
);
knowledge_id!(
    CodeIndexId,
    "code_index_",
    "Identity of one versioned code-intelligence index."
);

/// Opaque content-addressed token binding agent reads to one consistent view.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SnapshotToken(String);

impl SnapshotToken {
    /// Derive the token for a snapshot descriptor.
    #[must_use]
    pub fn for_descriptor(descriptor: &SnapshotDescriptor) -> Self {
        let source = descriptor
            .source_snapshot_id
            .as_ref()
            .map_or("none", SourceSnapshotId::as_str);
        let material = format!(
            "texo.snapshot-token.v1\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{source}",
            descriptor.workspace_id, descriptor.frontier, descriptor.anchor_event_id_hex
        );
        let digest = blake3_hash_hex(&material);
        Self(format!("texo_snap_{}", &digest[..32]))
    }

    /// Borrow the opaque token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SnapshotToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Descriptor bound by a [`SnapshotToken`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotDescriptor {
    /// Workspace being read.
    pub workspace_id: WorkspaceId,
    /// `BatPak` journal frontier included in the read.
    pub frontier: u64,
    /// Event id at `frontier`, or empty only for an empty journal.
    pub anchor_event_id_hex: String,
    /// Optional frozen Git/worktree evidence snapshot.
    pub source_snapshot_id: Option<SourceSnapshotId>,
}

/// Git object-hash algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitObjectFormat {
    /// SHA-1 object format.
    Sha1,
    /// SHA-256 object format.
    Sha256,
}

impl GitObjectFormat {
    const fn hex_len(self) -> usize {
        match self {
            Self::Sha1 => 40,
            Self::Sha256 => 64,
        }
    }
}

/// Typed Git object identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitObjectId {
    /// Hash algorithm used by the repository.
    pub format: GitObjectFormat,
    /// Lowercase hexadecimal object digest.
    pub hex: String,
}

impl GitObjectId {
    /// Validate and construct an object identity.
    ///
    /// # Errors
    /// Returns [`KnowledgeContractError::InvalidDigest`] for the wrong length
    /// or non-lowercase-hexadecimal input.
    pub fn new(
        format: GitObjectFormat,
        hex: impl Into<String>,
    ) -> Result<Self, KnowledgeContractError> {
        let hex = hex.into();
        let length = format.hex_len();
        validate_lower_hex("git object id", &hex, length)?;
        Ok(Self { format, hex })
    }
}

/// Half-open byte range into exact source bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByteRange {
    /// Inclusive start offset.
    pub start: u64,
    /// Exclusive end offset.
    pub end: u64,
}

impl ByteRange {
    /// Construct a non-reversed range.
    ///
    /// # Errors
    /// Returns [`KnowledgeContractError::ReversedRange`] when `start > end`.
    pub const fn new(start: u64, end: u64) -> Result<Self, KnowledgeContractError> {
        if start > end {
            return Err(KnowledgeContractError::ReversedRange { start, end });
        }
        Ok(Self { start, end })
    }
}

/// Relationship between two source-domain coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalRelation {
    /// Both coordinates name the same source state.
    Same,
    /// The left coordinate precedes the right.
    Before,
    /// The left coordinate follows the right.
    After,
    /// Both coordinates are valid but incomparable.
    Concurrent,
    /// Available evidence cannot determine an order.
    Unknown,
}

/// Quality of code-intelligence evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisQuality {
    /// Compiler or language-server index with resolved symbol identity.
    Precise,
    /// Grammar-backed syntax analysis without full name resolution.
    Syntactic,
    /// Bounded text matching only.
    Lexical,
    /// No analysis was available for the source.
    Unavailable,
}

/// State of an answer after evidence triangulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerState {
    /// Sufficient in-scope evidence supports the assertion.
    Supported,
    /// In-scope evidence contains an authoritative contradiction.
    Contradicted,
    /// The assertion has a comparable authoritative replacement.
    Stale,
    /// No sufficient supporting evidence was found within declared coverage.
    Unverified,
    /// Evidence exists on incomparable source revisions.
    Incomparable,
}

/// Source class for an evidence occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSourceKind {
    /// Markdown prose.
    Markdown,
    /// A raw committed Git blob.
    GitBlob,
    /// Frozen index or worktree bytes over a base commit.
    WorktreeOverlay,
    /// Compiler-precise SCIP occurrence.
    Scip,
    /// Grammar-backed syntactic occurrence.
    Syntax,
    /// Bounded lexical occurrence.
    Lexical,
}

/// Closed explanation for a coverage gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageGapKind {
    /// Configured byte or item budget was reached.
    BudgetExceeded,
    /// Git history is shallow.
    ShallowHistory,
    /// Referenced object is unavailable.
    MissingObject,
    /// Entry is a submodule gitlink.
    Gitlink,
    /// Blob is a Git LFS pointer whose target was not read.
    LfsPointer,
    /// Source encoding is unsupported.
    UnsupportedEncoding,
    /// Source exceeded a configured per-item size bound.
    SourceTooLarge,
    /// Index or parser does not support the language.
    UnsupportedLanguage,
    /// Source contains unresolved index/worktree conflict stages.
    WorktreeConflict,
    /// Analyzer reported recovery or incomplete semantic information.
    AnalysisIncomplete,
}

/// One bounded coverage omission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageGap {
    /// Repository-relative path when one source is affected.
    pub path: Option<String>,
    /// Closed omission class.
    pub kind: CoverageGapKind,
}

/// Coverage carried by every knowledge result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeCoverage {
    /// Strongest analysis quality actually used.
    pub analysis_quality: AnalysisQuality,
    /// Number of source items examined.
    pub sources_examined: u64,
    /// Number of evidence occurrences returned or recorded.
    pub occurrences: u64,
    /// Whether the operation stopped at a configured bound.
    pub truncated: bool,
    /// Typed omissions; empty only when no known gap exists.
    pub gaps: Vec<CoverageGap>,
}

/// Exact bounded evidence occurrence suitable for durable explanation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceOccurrence {
    /// Content-addressed occurrence identity.
    pub occurrence_id: EvidenceOccurrenceId,
    /// Frozen source snapshot containing the occurrence.
    pub snapshot_id: SourceSnapshotId,
    /// Evidence source class.
    pub source_kind: EvidenceSourceKind,
    /// Repository-relative path.
    pub path: String,
    /// Exact byte range in the captured source.
    pub byte_range: ByteRange,
    /// Optional committed blob identity.
    pub git_blob: Option<GitObjectId>,
    /// Digest of the exact complete source bytes.
    pub source_digest_hex: String,
    /// Exact bounded bytes rendered losslessly as UTF-8 evidence.
    pub excerpt: String,
    /// Extractor, parser, or indexer identity and version.
    pub analyzer_fingerprint: String,
    /// Quality tier of the analysis that produced the occurrence.
    pub analysis_quality: AnalysisQuality,
}

impl EvidenceOccurrence {
    /// Validate the bounded fields of an occurrence.
    ///
    /// # Errors
    /// Returns a contract error for an invalid source digest or oversized
    /// excerpt.
    pub fn validate(&self) -> Result<(), KnowledgeContractError> {
        validate_lower_hex("source digest", &self.source_digest_hex, 64)?;
        let actual = self.excerpt.len();
        if actual > MAX_EVIDENCE_EXCERPT_BYTES {
            return Err(KnowledgeContractError::ExcerptTooLarge {
                actual,
                maximum: MAX_EVIDENCE_EXCERPT_BYTES,
            });
        }
        Ok(())
    }
}

fn validate_lower_hex(
    field: &'static str,
    value: &str,
    length: usize,
) -> Result<(), KnowledgeContractError> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(KnowledgeContractError::InvalidDigest { field, length });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> SnapshotDescriptor {
        SnapshotDescriptor {
            workspace_id: WorkspaceId::new("demo").expect("workspace"),
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
}
