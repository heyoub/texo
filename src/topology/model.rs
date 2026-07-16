//! Topology validation failures.

/// Invalid topology declaration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TopologyError {
    /// No journals were declared.
    #[error("workspace topology declares no journals")]
    Empty,
    /// Invalid stable id.
    #[error("invalid journal id: {0}")]
    InvalidJournalId(String),
    /// Selected or primary journal is absent.
    #[error("journal is not declared: {0}")]
    MissingJournal(String),
    /// Primary journal must own authority.
    #[error("primary journal must be canonical: {0}")]
    PrimaryNotCanonical(String),
    /// Store path is empty.
    #[error("journal has an empty store path: {0}")]
    EmptyStorePath(String),
    /// Canonical journal carried replica-only fields.
    #[error("canonical journal carries replica fields: {0}")]
    CanonicalHasReplicaFields(String),
    /// Replica omitted source.
    #[error("replica is missing source_journal: {0}")]
    ReplicaMissingSource(String),
    /// Replica omitted materialization semantics.
    #[error("replica is missing replica_mode: {0}")]
    ReplicaMissingMode(String),
    /// Remote source fields are incomplete or unsafe.
    #[error("replica has invalid remote source fields: {0}")]
    InvalidRemoteSource(String),
    /// Exact forks require direct source-store access.
    #[error("remote replica cannot use exact_fork mode: {0}")]
    RemoteExactFork(String),
    /// Replica source is not declared.
    #[error("replica {replica} references missing source journal {source_journal}")]
    MissingSource {
        /// Replica id.
        replica: String,
        /// Missing source id.
        source_journal: String,
    },
    /// Replica graph contains a cycle.
    #[error("replica lineage contains a cycle at {0}")]
    ReplicaCycle(String),
    /// Two identities alias one configured data directory.
    #[error("journals {first} and {second} share store path {path}")]
    DuplicateStorePath {
        /// First journal id.
        first: String,
        /// Second journal id.
        second: String,
        /// Aliased path.
        path: String,
    },
}
