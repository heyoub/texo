//! Crate-wide texo error types.

use std::fmt;

use serde::Serialize;

mod detail {
    use std::error::Error;

    /// Render an error and every source in its causal chain.
    #[must_use]
    pub fn error_chain(error: &(dyn Error + 'static)) -> String {
        let mut rendered = error.to_string();
        let mut source = error.source();
        while let Some(cause) = source {
            rendered.push_str(": ");
            rendered.push_str(&cause.to_string());
            source = cause.source();
        }
        rendered
    }
}

pub use detail::error_chain;

/// Surface families that can report transport-bound errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    /// HTTP surface.
    Http,
    /// MCP surface.
    Mcp,
    /// CLI surface.
    Cli,
}

/// Whether durable work was committed before a failure surfaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Committed {
    /// No durable work was committed.
    No,
    /// Some durable work was committed.
    Partial,
    /// The requested durable fact was committed.
    Yes,
    /// The boundary cannot prove commit state.
    Unknown,
}

impl fmt::Display for Committed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::No => "no",
            Self::Partial => "partial",
            Self::Yes => "yes",
            Self::Unknown => "unknown",
        })
    }
}

/// Machine-readable recovery facts shared by every surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FailureFacts {
    /// Commit state at the failure boundary.
    pub committed: Committed,
    /// Whether replaying the identical request is safe.
    pub retry_safe: bool,
    /// Stable recovery instruction when one exists.
    pub resume: Option<&'static str>,
}

/// Closed snapshot-read failure class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotFailureKind {
    /// Token shape or checksum is invalid.
    InvalidToken,
    /// The exact journal frontier is no longer readable.
    Unavailable,
    /// The recorded anchor does not match the journal entry at the frontier.
    AnchorMismatch,
    /// The token names a source snapshot not present in this store.
    SourceUnavailable,
}

/// Closed replica-circuit failure class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplicationFailureKind {
    /// Topology does not describe the requested source/replica circuit.
    InvalidTopology,
    /// A destructive bootstrap was refused because the destination has bytes.
    DestinationNotFresh,
    /// The configured replica mode cannot perform the requested operation.
    ModeMismatch,
    /// A persisted source cursor no longer names the same source event.
    AnchorMismatch,
    /// The materialized replica failed a source-truth verification check.
    Verification,
    /// Durable circuit evidence could not be loaded or persisted.
    Evidence,
    /// `BatPak` returned a lifecycle or import failure at an uncertain boundary.
    Substrate,
    /// A bounded remote replica call failed before its response was accepted.
    Transport,
    /// Another process currently owns one physical journal directory.
    Busy,
}

impl ReplicationFailureKind {
    const fn code(self) -> &'static str {
        match self {
            Self::InvalidTopology => "replication.topology",
            Self::DestinationNotFresh => "replication.destination_not_fresh",
            Self::ModeMismatch => "replication.mode",
            Self::AnchorMismatch => "replication.anchor",
            Self::Verification => "replication.verify",
            Self::Evidence => "replication.evidence",
            Self::Substrate => "replication.substrate",
            Self::Transport => "replication.transport",
            Self::Busy => "replication.busy",
        }
    }
}

impl SnapshotFailureKind {
    const fn code(self) -> &'static str {
        match self {
            Self::InvalidToken => "snapshot.invalid",
            Self::Unavailable => "snapshot.unavailable",
            Self::AnchorMismatch => "snapshot.anchor",
            Self::SourceUnavailable => "snapshot.source_unavailable",
        }
    }
}

impl fmt::Display for SnapshotFailureKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidToken => "invalid token",
            Self::Unavailable => "unavailable",
            Self::AnchorMismatch => "anchor mismatch",
            Self::SourceUnavailable => "source unavailable",
        })
    }
}

impl SurfaceKind {
    fn code(self) -> &'static str {
        match self {
            Self::Http => "surface.http",
            Self::Mcp => "surface.mcp",
            Self::Cli => "surface.cli",
        }
    }
}

impl fmt::Display for SurfaceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Http => "http",
            Self::Mcp => "mcp",
            Self::Cli => "cli",
        })
    }
}

mod kind {
    use std::error::Error;

    use super::{
        replication_facts, Committed, FailureFacts, ReplicationFailureKind, SnapshotFailureKind,
        SurfaceKind,
    };

    /// Unified texo error with stable machine-readable codes.
    #[non_exhaustive]
    #[derive(Debug, thiserror::Error)]
    pub enum TexoError {
        /// Configuration failure.
        #[error("config: {detail}")]
        Config {
            /// Human-readable detail.
            detail: String,
            /// Optional underlying configuration source error.
            #[source]
            source: Option<Box<dyn Error + Send + Sync>>,
        },
        /// Filesystem I/O error.
        #[error("io: {0}")]
        Io(#[from] std::io::Error),
        /// JSON serialization or parsing error.
        #[error("json: {0}")]
        Json(#[from] serde_json::Error),
        /// `BatPak` store error.
        #[error("store: {0}")]
        Store(#[from] batpak::store::StoreError),
        /// `BatPak` coordinate construction failure.
        #[error("store coordinate: {detail}")]
        Coordinate {
            /// Human-readable detail.
            detail: String,
        },
        /// `BatPak` payload registry failure.
        #[error("store registry: {detail}")]
        Registry {
            /// Human-readable detail.
            detail: String,
        },
        /// Journal decode error for an entity stream.
        #[error("journal decode for {entity}: {detail}")]
        Decode {
            /// Entity stream that could not be decoded.
            entity: String,
            /// Decode failure detail.
            detail: String,
        },
        /// Append receipt verification failure.
        #[error("journal receipt for {event_id}: {reason}")]
        ReceiptInvalid {
            /// Event id whose append receipt failed verification.
            event_id: String,
            /// Rejection reason.
            reason: String,
        },
        /// Domain identifier parse error.
        #[error("domain id: {0}")]
        IdParse(#[from] crate::events::ids::IdParseError),
        /// Domain status parse error.
        #[error("domain status: {value}")]
        StatusParse {
            /// Unrecognized status string.
            value: String,
        },
        /// Illegal domain transition request.
        #[error("domain transition {machine}: {from} -> {to}{context_suffix}", context_suffix = context.as_deref().map(|value| format!(" ({value})")).unwrap_or_default())]
        Transition {
            /// State machine identifier.
            machine: String,
            /// Source state.
            from: u64,
            /// Destination state.
            to: u64,
            /// Typed entity/state context.
            context: Option<String>,
        },
        /// Required domain entity is absent.
        #[error("domain missing: {entity}")]
        MissingEntity {
            /// Missing entity stream.
            entity: String,
        },
        /// Source parsing or discovery error.
        #[error("source {path}: {detail}")]
        Source {
            /// Source path.
            path: String,
            /// Failure detail.
            detail: String,
        },
        /// Claim extraction error.
        #[error("extract: {detail}")]
        Extract {
            /// Failure detail.
            detail: String,
        },
        /// Semantic backend error.
        #[error("semantics {backend}: {detail}")]
        Semantics {
            /// Backend identifier.
            backend: String,
            /// Failure detail.
            detail: String,
        },
        /// Verification failures.
        #[error("verify: {failures:?}")]
        Verify {
            /// Verification failure rows.
            failures: Vec<String>,
        },
        /// Operation input decode or validation failure.
        #[error("op input {op}: {detail}")]
        OpInput {
            /// Operation name.
            op: String,
            /// Failure detail.
            detail: String,
        },
        /// Operation runtime failure or denial.
        #[error("op runtime {op}: {detail}")]
        OpRuntime {
            /// Operation name.
            op: String,
            /// Failure detail.
            detail: String,
            /// Whether the runtime denied execution.
            denied: bool,
        },
        /// Host composition or invocation failure.
        #[error("host: {detail}")]
        Host {
            /// Failure detail.
            detail: String,
        },
        /// Surface-layer error.
        #[error("surface {which}: {detail}")]
        Surface {
            /// Surface family.
            which: SurfaceKind,
            /// Failure detail.
            detail: String,
        },
        /// Model invocation error.
        #[error("model: {detail}")]
        Model {
            /// Failure detail.
            detail: String,
        },
        /// Session handling error.
        #[error("session: {detail}")]
        Session {
            /// Failure detail.
            detail: String,
        },
        /// Backup creation or verification environment failure.
        #[error("backup: {detail}")]
        Backup {
            /// Sanitized failure detail.
            detail: String,
        },
        /// Snapshot-consistent read failure.
        #[error("snapshot {kind}: {detail}")]
        Snapshot {
            /// Closed failure class.
            kind: SnapshotFailureKind,
            /// Sanitized failure detail.
            detail: String,
        },
        /// Replica bootstrap, follow, or evidence failure.
        #[error("replication {kind:?}: {detail}")]
        Replication {
            /// Closed circuit failure class.
            kind: ReplicationFailureKind,
            /// What was durably changed before the failure surfaced.
            committed: Committed,
            /// Sanitized diagnostic detail.
            detail: String,
        },
    }

    impl TexoError {
        /// Stable machine-readable error code.
        #[must_use]
        pub fn code(&self) -> &'static str {
            match self {
                Self::Config { .. } => "config",
                Self::Io(_) => "io",
                Self::Json(_) => "json",
                Self::Store(_) => "store",
                Self::Coordinate { .. } => "store.coordinate",
                Self::Registry { .. } => "store.registry",
                Self::Decode { .. } => "journal.decode",
                Self::ReceiptInvalid { .. } => "journal.receipt",
                Self::IdParse(_) => "domain.id",
                Self::StatusParse { .. } => "domain.status",
                Self::Transition { .. } => "domain.transition",
                Self::MissingEntity { .. } => "domain.missing",
                Self::Source { .. } => "source",
                Self::Extract { .. } => "extract",
                Self::Semantics { .. } => "semantics",
                Self::Verify { .. } => "verify",
                Self::OpInput { .. } => "op.input",
                Self::OpRuntime { denied, .. } => {
                    if *denied {
                        "op.denied"
                    } else {
                        "op.runtime"
                    }
                }
                Self::Host { .. } => "host",
                Self::Surface { which, .. } => which.code(),
                Self::Model { .. } => "agent.model",
                Self::Session { .. } => "agent.session",
                Self::Backup { .. } => "backup",
                Self::Snapshot { kind, .. } => kind.code(),
                Self::Replication { kind, .. } => kind.code(),
            }
        }

        /// Recovery facts for this failure.
        #[must_use]
        pub fn facts(&self) -> FailureFacts {
            use Committed::{No, Unknown, Yes};
            match self {
                Self::Config { .. }
                | Self::Json(_)
                | Self::Coordinate { .. }
                | Self::Registry { .. }
                | Self::IdParse(_)
                | Self::StatusParse { .. }
                | Self::Transition { .. }
                | Self::MissingEntity { .. }
                | Self::Source { .. }
                | Self::Extract { .. }
                | Self::Verify { .. }
                | Self::Backup { .. }
                | Self::OpRuntime { denied: true, .. } => FailureFacts {
                    committed: No,
                    retry_safe: false,
                    resume: None,
                },
                Self::Snapshot { kind, .. } => FailureFacts {
                    committed: No,
                    retry_safe: true,
                    resume: Some(match kind {
                        SnapshotFailureKind::InvalidToken => "fix the snapshot token and retry",
                        SnapshotFailureKind::Unavailable
                        | SnapshotFailureKind::AnchorMismatch
                        | SnapshotFailureKind::SourceUnavailable => {
                            "request the latest workspace snapshot and retry"
                        }
                    }),
                },
                Self::Replication {
                    kind, committed, ..
                } => replication_facts(*kind, *committed),
                Self::OpInput { .. } => FailureFacts {
                    committed: No,
                    retry_safe: true,
                    resume: Some("fix the input and retry"),
                },
                Self::Semantics { .. } => FailureFacts {
                    committed: No,
                    retry_safe: true,
                    resume: Some("run `texo relate` to resume unresolved pairs"),
                },
                Self::Model { .. } => FailureFacts {
                    committed: Yes,
                    retry_safe: false,
                    resume: Some("user turn already recorded; re-sending duplicates it"),
                },
                Self::OpRuntime { op, detail, .. }
                    if op == "texo.agent.chat" && detail.contains("agent.model") =>
                {
                    FailureFacts {
                        committed: Yes,
                        retry_safe: false,
                        resume: Some("user turn already recorded; re-sending duplicates it"),
                    }
                }
                Self::OpRuntime { op, detail, .. }
                    if op == "texo.ingest.run" && detail.contains("source") =>
                {
                    FailureFacts {
                        committed: No,
                        retry_safe: false,
                        resume: None,
                    }
                }
                Self::OpRuntime { op, detail, .. }
                    if matches!(
                        op.as_str(),
                        "texo.claim.supersede" | "texo.conflict.resolve"
                    ) && detail.contains("domain.transition") =>
                {
                    FailureFacts {
                        committed: No,
                        retry_safe: false,
                        resume: None,
                    }
                }
                Self::Io(_)
                | Self::Store(_)
                | Self::Decode { .. }
                | Self::ReceiptInvalid { .. }
                | Self::OpRuntime { .. }
                | Self::Host { .. }
                | Self::Surface { .. }
                | Self::Session { .. } => FailureFacts {
                    committed: Unknown,
                    retry_safe: false,
                    resume: Some("inspect receipts and run `texo verify` before retrying"),
                },
            }
        }
    }
}

pub use kind::TexoError;

const fn replication_facts(kind: ReplicationFailureKind, committed: Committed) -> FailureFacts {
    let resume = match kind {
        ReplicationFailureKind::InvalidTopology
        | ReplicationFailureKind::DestinationNotFresh
        | ReplicationFailureKind::ModeMismatch => "fix the replica topology and retry",
        ReplicationFailureKind::AnchorMismatch => {
            "inspect the source journal identity before resuming"
        }
        ReplicationFailureKind::Verification => {
            "verify the replica and rebuild it from its canonical source"
        }
        ReplicationFailureKind::Evidence => "retry to persist the replica evidence cursor",
        ReplicationFailureKind::Substrate => {
            "inspect both journal frontiers and receipts before retrying"
        }
        ReplicationFailureKind::Transport => "resume the replica from its durable cursor",
        ReplicationFailureKind::Busy => "retry after the current journal owner releases it",
    };
    FailureFacts {
        committed,
        retry_safe: matches!(
            kind,
            ReplicationFailureKind::Evidence
                | ReplicationFailureKind::Transport
                | ReplicationFailureKind::Busy
        ),
        resume: Some(resume),
    }
}

impl From<TexoError> for syncbat::HandlerError {
    fn from(error: TexoError) -> Self {
        let code = error.code();
        let is_input = matches!(error, TexoError::OpInput { .. });
        let message = format!("{code}: {error}");
        if is_input {
            Self::InvalidInput(message)
        } else {
            Self::Failed(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use batpak::id::EventId;
    use syncbat::HandlerError;

    use super::*;

    fn config_error() -> TexoError {
        TexoError::Config {
            detail: "bad".to_string(),
            source: None,
        }
    }

    fn assert_codes<const N: usize>(cases: [(TexoError, &str); N]) {
        for (error, expected) in cases {
            assert_eq!(error.code(), expected);
        }
    }

    #[test]
    fn storage_and_journal_codes_match_the_public_table() {
        assert_codes([
            (config_error(), "config"),
            (TexoError::Io(std::io::Error::other("io")), "io"),
            (
                TexoError::Json(
                    serde_json::from_str::<serde_json::Value>("not-json")
                        .expect_err("invalid json creates an error"),
                ),
                "json",
            ),
            (
                TexoError::Store(batpak::store::StoreError::NotFound(EventId::from_u128(1))),
                "store",
            ),
            (
                TexoError::Coordinate {
                    detail: "bad coordinate".to_string(),
                },
                "store.coordinate",
            ),
            (
                TexoError::Registry {
                    detail: "registry".to_string(),
                },
                "store.registry",
            ),
            (
                TexoError::Decode {
                    entity: "claim:claim_a".to_string(),
                    detail: "bad payload".to_string(),
                },
                "journal.decode",
            ),
            (
                TexoError::ReceiptInvalid {
                    event_id: "1".to_string(),
                    reason: "bad".to_string(),
                },
                "journal.receipt",
            ),
        ]);
    }

    #[test]
    fn domain_and_source_codes_match_the_public_table() {
        assert_codes([
            (
                TexoError::IdParse(crate::events::ids::IdParseError::Empty),
                "domain.id",
            ),
            (
                TexoError::StatusParse {
                    value: "stale".to_string(),
                },
                "domain.status",
            ),
            (
                TexoError::Transition {
                    machine: "m".to_string(),
                    from: 2,
                    to: 1,
                    context: None,
                },
                "domain.transition",
            ),
            (
                TexoError::MissingEntity {
                    entity: "claim:missing".to_string(),
                },
                "domain.missing",
            ),
            (
                TexoError::Source {
                    path: "a.md".to_string(),
                    detail: "bad".to_string(),
                },
                "source",
            ),
            (
                TexoError::Extract {
                    detail: "bad".to_string(),
                },
                "extract",
            ),
            (
                TexoError::Semantics {
                    backend: "none".to_string(),
                    detail: "bad".to_string(),
                },
                "semantics",
            ),
            (
                TexoError::Verify {
                    failures: vec!["bad".to_string()],
                },
                "verify",
            ),
        ]);
    }

    #[test]
    fn operation_and_surface_codes_match_the_public_table() {
        assert_codes([
            (
                TexoError::OpInput {
                    op: "op".to_string(),
                    detail: "bad".to_string(),
                },
                "op.input",
            ),
            (
                TexoError::OpRuntime {
                    op: "op".to_string(),
                    detail: "bad".to_string(),
                    denied: false,
                },
                "op.runtime",
            ),
            (
                TexoError::OpRuntime {
                    op: "op".to_string(),
                    detail: "bad".to_string(),
                    denied: true,
                },
                "op.denied",
            ),
            (
                TexoError::Host {
                    detail: "bad".to_string(),
                },
                "host",
            ),
            (
                TexoError::Surface {
                    which: SurfaceKind::Http,
                    detail: "bad".to_string(),
                },
                "surface.http",
            ),
            (
                TexoError::Surface {
                    which: SurfaceKind::Mcp,
                    detail: "bad".to_string(),
                },
                "surface.mcp",
            ),
            (
                TexoError::Surface {
                    which: SurfaceKind::Cli,
                    detail: "bad".to_string(),
                },
                "surface.cli",
            ),
        ]);
    }

    #[test]
    fn agent_and_durability_codes_match_the_public_table() {
        assert_codes([
            (
                TexoError::Model {
                    detail: "bad".to_string(),
                },
                "agent.model",
            ),
            (
                TexoError::Session {
                    detail: "bad".to_string(),
                },
                "agent.session",
            ),
            (
                TexoError::Backup {
                    detail: "bad".to_string(),
                },
                "backup",
            ),
            (
                TexoError::Snapshot {
                    kind: SnapshotFailureKind::InvalidToken,
                    detail: "bad".to_string(),
                },
                "snapshot.invalid",
            ),
        ]);
    }

    #[test]
    fn handler_error_mapping_preserves_codes() {
        let input: HandlerError = TexoError::OpInput {
            op: "texo.op".to_string(),
            detail: "bad".to_string(),
        }
        .into();
        assert_eq!(input.class(), "invalid_input");
        assert!(input.message().starts_with("op.input: "));

        let runtime: HandlerError = TexoError::OpRuntime {
            op: "texo.op".to_string(),
            detail: "bad".to_string(),
            denied: false,
        }
        .into();
        assert_eq!(runtime.class(), "failed");
        assert!(runtime.message().starts_with("op.runtime: "));
    }

    #[test]
    fn chat_model_failure_discloses_committed_turn() {
        let error = TexoError::OpRuntime {
            op: "texo.agent.chat".to_string(),
            detail: "failed: agent.model: provider timeout".to_string(),
            denied: false,
        };
        assert_eq!(
            error.facts(),
            FailureFacts {
                committed: Committed::Yes,
                retry_safe: false,
                resume: Some("user turn already recorded; re-sending duplicates it"),
            }
        );
    }
}
