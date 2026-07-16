//! Scale-out replica circuits composed from BatPak lifecycle and import APIs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::WorkspaceConfig;
use crate::topology::ResolvedJournal;

pub(super) const STATE_SCHEMA_VERSION: u32 = 2;

/// Durable operational cursor for one imported read model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicaCursor {
    /// Cursor schema.
    pub schema_version: u32,
    /// Logical workspace scope.
    pub workspace_id: String,
    /// Stable source journal id.
    pub source_journal: String,
    /// Stable destination journal id.
    pub replica_journal: String,
    /// Stable import namespace used in destination idempotency keys.
    pub source_namespace: String,
    /// Remote source address, or absent for a same-host circuit.
    pub source_endpoint: Option<String>,
    /// Highest source sequence fully observed by the last successful call.
    pub source_high_watermark: Option<u64>,
    /// Event id at the source cursor, used to reject truncation/store swap.
    pub source_anchor_event_id_hex: Option<String>,
    /// Destination visible frontier after the last successful call.
    pub replica_frontier: u64,
    /// Event id at the destination frontier after the last successful call.
    pub replica_anchor_event_id_hex: Option<String>,
}

/// Exact point-in-time fork evidence retained outside the copied store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExactForkEvidence {
    /// Evidence schema.
    pub schema_version: u32,
    /// Logical workspace scope.
    pub workspace_id: String,
    /// Stable source journal id.
    pub source_journal: String,
    /// Stable destination journal id.
    pub replica_journal: String,
    /// `BatPak` structural fork identity.
    pub fork_id_hex: String,
    /// Canonical `BatPak` fork report hash.
    pub report_hash_hex: String,
    /// Source visible frontier at the fork boundary.
    pub source_frontier: u64,
    /// Fork visible frontier after reopen.
    pub replica_frontier: u64,
    /// Number of events verified in the reopened fork.
    pub events_verified: usize,
}

/// Machine-readable result of one replica operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ReplicaReport {
    /// Identity-preserving point-in-time clone.
    ExactFork {
        /// Persisted structural evidence.
        evidence: ExactForkEvidence,
        /// Workspace-relative evidence artifact.
        evidence_path: String,
    },
    /// Destination-local imported read model.
    ImportedReadModel {
        /// Number of source events newly materialized.
        imported: u64,
        /// Number already present by deterministic import identity.
        deduplicated: u64,
        /// Number of substrate-reserved events intentionally omitted.
        skipped_reserved: u64,
        /// Replica-ledger events omitted when following another replica.
        skipped_operational: u64,
        /// Cursor persisted after verification.
        cursor: ReplicaCursor,
        /// Workspace-relative cursor artifact.
        cursor_path: String,
    },
}

struct Circuit {
    workspace: WorkspaceConfig,
    source: ResolvedJournal,
    replica: ResolvedJournal,
    source_path: PathBuf,
    replica_path: PathBuf,
}

mod evidence;
mod lifecycle;
mod materialize;
mod remote;

pub use lifecycle::{bootstrap, follow_once, refresh_reader};
