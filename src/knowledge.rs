//! Frozen contracts for snapshot-consistent evidence and code knowledge.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::events::ids::{blake3_hash_hex, WorkspaceId};
use crate::topology::JournalId;

mod contracts;

pub use contracts::{KnowledgeContractError, KnowledgeCoverage};

/// Maximum exact evidence excerpt carried by one durable occurrence.
pub const MAX_EVIDENCE_EXCERPT_BYTES: usize = 4 * 1024;

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

            /// Parse a previously emitted identifier.
            ///
            /// # Errors
            /// Returns [`KnowledgeContractError::InvalidIdentifier`] when the
            /// prefix or character set is invalid.
            pub fn parse(value: &str) -> Result<Self, KnowledgeContractError> {
                if !value.starts_with($prefix)
                    || value.len() <= $prefix.len()
                    || !value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
                {
                    return Err(KnowledgeContractError::InvalidIdentifier {
                        kind: stringify!($name),
                    });
                }
                Ok(Self(value.to_string()))
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
        let anchor = if descriptor.anchor_event_id_hex.is_empty() {
            "-"
        } else {
            descriptor.anchor_event_id_hex.as_str()
        };
        let digest = snapshot_token_digest(
            &descriptor.workspace_id,
            &descriptor.journal_id,
            descriptor.frontier,
            anchor,
            source,
        );
        Self(format!(
            "texo_snap_v2.{}.{}.{}.{}",
            descriptor.frontier,
            anchor,
            source,
            &digest[..32]
        ))
    }

    /// Borrow the opaque token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse an untrusted token and recover its bound descriptor.
    ///
    /// Workspace and journal identities are external to the token so copying a
    /// token across either boundary fails checksum validation.
    ///
    /// # Errors
    /// Returns [`KnowledgeContractError::InvalidIdentifier`] for an invalid
    /// shape, field, or checksum.
    pub fn resolve_for_journal(
        value: &str,
        workspace_id: &WorkspaceId,
        journal_id: &JournalId,
    ) -> Result<SnapshotDescriptor, KnowledgeContractError> {
        let invalid = || KnowledgeContractError::InvalidIdentifier {
            kind: "SnapshotToken",
        };
        let mut fields = value.split('.');
        if fields.next() != Some("texo_snap_v2") {
            return Err(invalid());
        }
        let frontier = fields
            .next()
            .ok_or_else(&invalid)?
            .parse::<u64>()
            .map_err(|_| invalid())?;
        let anchor = fields.next().ok_or_else(&invalid)?;
        if anchor != "-" {
            validate_lower_hex("snapshot anchor", anchor, 32)?;
        }
        let source = fields.next().ok_or_else(&invalid)?;
        let checksum = fields.next().ok_or_else(&invalid)?;
        if fields.next().is_some() {
            return Err(invalid());
        }
        validate_lower_hex("snapshot token checksum", checksum, 32)?;
        let expected = snapshot_token_digest(workspace_id, journal_id, frontier, anchor, source);
        if checksum != &expected[..32] {
            return Err(invalid());
        }
        let source_snapshot_id = if source == "none" {
            None
        } else {
            Some(SourceSnapshotId::parse(source)?)
        };
        Ok(SnapshotDescriptor {
            workspace_id: workspace_id.clone(),
            journal_id: journal_id.clone(),
            frontier,
            anchor_event_id_hex: if anchor == "-" {
                String::new()
            } else {
                anchor.to_string()
            },
            source_snapshot_id,
        })
    }
}

impl fmt::Display for SnapshotToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn snapshot_token_digest(
    workspace_id: &WorkspaceId,
    journal_id: &JournalId,
    frontier: u64,
    anchor: &str,
    source: &str,
) -> String {
    blake3_hash_hex(&format!(
        "texo.snapshot-token.v2\u{1f}{workspace_id}\u{1f}{journal_id}\u{1f}{frontier}\u{1f}{anchor}\u{1f}{source}"
    ))
}

/// Descriptor bound by a [`SnapshotToken`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotDescriptor {
    /// Workspace being read.
    pub workspace_id: WorkspaceId,
    /// Physical journal whose local sequence/anchor coordinates are bound.
    pub journal_id: JournalId,
    /// `BatPak` journal frontier included in the read.
    pub frontier: u64,
    /// Event id at `frontier`, or empty only for an empty journal.
    pub anchor_event_id_hex: String,
    /// Optional frozen Git/worktree evidence snapshot.
    pub source_snapshot_id: Option<SourceSnapshotId>,
}

/// Snapshot identity returned by every agent-facing read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRead {
    /// Opaque token to pass to subsequent reads.
    pub token: SnapshotToken,
    /// Coordinates bound by the token.
    pub descriptor: SnapshotDescriptor,
}

impl SnapshotRead {
    /// Construct a read identity from its descriptor.
    #[must_use]
    pub fn new(descriptor: SnapshotDescriptor) -> Self {
        let token = SnapshotToken::for_descriptor(&descriptor);
        Self { token, descriptor }
    }
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

/// Inclusive one-based source line range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineRange {
    /// Inclusive first line.
    pub start: u32,
    /// Inclusive last line.
    pub end: u32,
}

impl LineRange {
    /// Construct a non-reversed, one-based line range.
    ///
    /// # Errors
    /// Returns a range error when either line is zero or `start > end`.
    pub fn new(start: u32, end: u32) -> Result<Self, KnowledgeContractError> {
        if start == 0 || end == 0 || start > end {
            return Err(KnowledgeContractError::InvalidLineRange { start, end });
        }
        Ok(Self { start, end })
    }
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

/// Closed target accepted by evidence triangulation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TriangulationTarget {
    /// One durable semantic assertion.
    Claim {
        /// Claim identity.
        claim_id: String,
    },
    /// All assertions and evidence intersecting a repository path and optional
    /// one-based inclusive line range.
    Path {
        /// Slash-separated repository-relative path.
        path: String,
        /// Optional first line.
        line_start: Option<u32>,
        /// Optional last line.
        line_end: Option<u32>,
    },
    /// One language-index symbol. Precise resolution requires an indexed code
    /// artifact and never falls back silently.
    Symbol {
        /// SCIP or analyzer-stable symbol identifier.
        symbol: String,
    },
}

/// Typed reason an evidence answer cannot be treated as complete certainty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyReason {
    /// No frozen source snapshot exists at the requested journal frontier.
    SourceSnapshotUnavailable,
    /// The snapshot has known coverage omissions or hit a configured bound.
    PartialCoverage,
    /// Relevant semantic pair settlement is incomplete.
    SettlementIncomplete,
    /// The target requires a code index that is not present at this snapshot.
    CodeIndexUnavailable,
    /// Evidence exists on source revisions that are not ancestrally ordered.
    ConcurrentRevision,
    /// The target exists as an assertion but has no exact durable occurrence.
    ExactEvidenceUnavailable,
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

/// How one evidence occurrence bears on a semantic assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStance {
    /// Evidence directly supports the assertion.
    Supports,
    /// Evidence contradicts the assertion.
    Contradicts,
    /// Evidence mentions the subject without deciding the assertion.
    Mentions,
}

/// Mechanism that produced an evidence-to-claim link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceLinkMethod {
    /// Exact source identity and span match.
    Deterministic,
    /// Cached model proposal accepted by deterministic policy.
    SemanticPolicy,
}

/// Durable code-index artifact format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeIndexFormat {
    /// SCIP Protocol Buffer index.
    Scip,
    /// Texo's bounded syntactic index.
    Syntax,
    /// Texo's bounded lexical index.
    Lexical,
}

/// Closed role of one code-symbol occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeOccurrenceRole {
    /// Symbol definition.
    Definition,
    /// Ordinary symbol reference.
    Reference,
    /// Import occurrence.
    Import,
    /// Write access.
    Write,
    /// Read access.
    Read,
    /// Generated source.
    Generated,
    /// Test source.
    Test,
    /// Forward definition.
    ForwardDefinition,
    /// Implementation relationship or declaration.
    Implementation,
}

/// One exact code-symbol occurrence from a disposable code index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeOccurrence {
    /// Analyzer-stable symbol identity.
    pub symbol: String,
    /// Human-readable unqualified spelling.
    pub display_name: String,
    /// Closed occurrence roles.
    pub roles: Vec<CodeOccurrenceRole>,
    /// Repository-relative source path.
    pub path: String,
    /// Exact half-open byte range in the frozen source.
    pub byte_range: ByteRange,
    /// Exact one-based source line range.
    pub line_range: LineRange,
    /// Digest of the complete frozen source bytes.
    pub source_digest_hex: String,
    /// Exact bounded source text at the occurrence.
    pub excerpt: String,
    /// Bounded surrounding source context used for evidence reconciliation.
    #[serde(default)]
    pub context: String,
    /// Exact byte range of `context` in the frozen source.
    #[serde(default = "empty_byte_range")]
    pub context_byte_range: ByteRange,
    /// Exact line range of `context` in the frozen source.
    #[serde(default = "first_line_range")]
    pub context_line_range: LineRange,
    /// Analyzer implementation and version.
    pub analyzer_fingerprint: String,
    /// Precision tier of this occurrence.
    pub analysis_quality: AnalysisQuality,
}

const fn empty_byte_range() -> ByteRange {
    ByteRange { start: 0, end: 0 }
}

const fn first_line_range() -> LineRange {
    LineRange { start: 1, end: 1 }
}

/// Content-addressed disposable code-index artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeIndexArtifact {
    /// Artifact format version.
    pub schema: String,
    /// Frozen source snapshot indexed.
    pub snapshot_id: SourceSnapshotId,
    /// Content-addressed index identity.
    pub index_id: CodeIndexId,
    /// Strongest analyzer format included.
    pub format: CodeIndexFormat,
    /// Analyzer identity and version.
    pub analyzer_fingerprint: String,
    /// Sorted deterministic occurrences.
    pub occurrences: Vec<CodeOccurrence>,
    /// Honest coverage and omissions.
    pub coverage: KnowledgeCoverage,
}

/// Closed explanation for a coverage gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageGapKind {
    /// No durable Git/worktree source snapshot has been indexed yet.
    SourceSnapshotUnavailable,
    /// No authenticated code-index artifact is available at this snapshot.
    CodeIndexUnavailable,
    /// Configured byte or item budget was reached.
    BudgetExceeded,
    /// Git history is shallow.
    ShallowHistory,
    /// Referenced object is unavailable.
    MissingObject,
    /// Entry is a submodule gitlink.
    Gitlink,
    /// Entry is a symbolic link; it was excluded and its target was never followed.
    Symlink,
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
    /// Tracked file exists but its type is outside the capture/index scope, so it
    /// was not indexed. Recorded so its absence reads as "not indexed" rather than
    /// "not present".
    OutOfScope,
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
    /// Exact one-based line range.
    pub line_range: LineRange,
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

/// Snapshot-bounded evidence joined to one durable semantic assertion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimEvidence {
    /// Assertion receiving the evidence link.
    pub claim_id: String,
    /// Exact durable occurrence.
    pub occurrence: EvidenceOccurrence,
    /// How the evidence bears on the assertion.
    pub stance: EvidenceStance,
    /// Mechanism that linked evidence to the assertion.
    pub method: EvidenceLinkMethod,
    /// Journal sequence of the occurrence event.
    pub occurrence_sequence: u64,
    /// Journal sequence of the link event.
    pub link_sequence: u64,
    /// Accepted model/policy provenance for semantic links.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciliation: Option<ReconciliationProvenance>,
}

/// Durable provenance explaining one policy-accepted semantic evidence link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationProvenance {
    /// Model score in integer parts per million.
    pub score_ppm: u32,
    /// Provider/model/prompt identity.
    pub judge_fingerprint: String,
    /// Content-addressed paid-result cache key.
    pub cache_key_hex: String,
    /// Deterministic acceptance policy version.
    pub policy_version: String,
    /// Observation time retained as provenance, never ordering.
    pub observed_at_ms: u64,
    /// Journal sequence of the acceptance fact.
    pub acceptance_sequence: u64,
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
mod tests;
