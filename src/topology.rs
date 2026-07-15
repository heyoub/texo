//! Explicit multi-journal topology and role contracts.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Stable workspace-local identity of one physical `BatPak` journal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JournalId(String);

impl JournalId {
    /// Validate and construct a journal id.
    ///
    /// # Errors
    /// Returns [`TopologyError::InvalidJournalId`] for empty, path-like, or
    /// non-portable identifiers.
    pub fn new(value: impl Into<String>) -> Result<Self, TopologyError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(TopologyError::InvalidJournalId(value));
        }
        Ok(Self(value))
    }

    /// Borrow the stable id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for JournalId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Authority role of a physical journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalRole {
    /// Authoritative ordered write log for its configured coordinate space.
    Canonical,
    /// Derived local read model following another journal.
    Replica,
}

/// Replica materialization contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplicaMode {
    /// Identity-preserving point-in-time fork; promotion still requires policy.
    ExactFork,
    /// Import-provenance read model with destination-local event identities.
    ImportedReadModel,
}

/// One journal declaration inside a workspace topology.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalEntry {
    /// Physical role.
    pub role: JournalRole,
    /// Relative or absolute `BatPak` data directory.
    pub store_path: String,
    /// Upstream journal id for replicas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_journal: Option<String>,
    /// Replica identity/materialization semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replica_mode: Option<ReplicaMode>,
    /// Optional `netbat` source address for a remote replica circuit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_endpoint: Option<String>,
    /// Environment variable containing the remote circuit bearer token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_token_env: Option<String>,
}

impl JournalEntry {
    /// Declare an authoritative journal.
    #[must_use]
    pub fn canonical(store_path: impl Into<String>) -> Self {
        Self {
            role: JournalRole::Canonical,
            store_path: store_path.into(),
            source_journal: None,
            replica_mode: None,
            source_endpoint: None,
            source_token_env: None,
        }
    }

    /// Declare a derived replica.
    #[must_use]
    pub fn replica(
        store_path: impl Into<String>,
        source_journal: impl Into<String>,
        replica_mode: ReplicaMode,
    ) -> Self {
        Self {
            role: JournalRole::Replica,
            store_path: store_path.into(),
            source_journal: Some(source_journal.into()),
            replica_mode: Some(replica_mode),
            source_endpoint: None,
            source_token_env: None,
        }
    }

    /// Declare a remote imported read model served over `netbat`.
    #[must_use]
    pub fn remote_replica(
        store_path: impl Into<String>,
        source_journal: impl Into<String>,
        source_endpoint: impl Into<String>,
        source_token_env: impl Into<String>,
    ) -> Self {
        Self {
            role: JournalRole::Replica,
            store_path: store_path.into(),
            source_journal: Some(source_journal.into()),
            replica_mode: Some(ReplicaMode::ImportedReadModel),
            source_endpoint: Some(source_endpoint.into()),
            source_token_env: Some(source_token_env.into()),
        }
    }
}

/// Validated selected journal used by the host and snapshot contracts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedJournal {
    /// Stable workspace-local id.
    pub id: JournalId,
    /// Authority role.
    pub role: JournalRole,
    /// Relative or absolute data directory.
    pub store_path: String,
    /// Validated upstream identity for a replica.
    pub source_journal: Option<JournalId>,
    /// Replica semantics, absent for canonical journals.
    pub replica_mode: Option<ReplicaMode>,
    /// Optional remote `netbat` address.
    pub source_endpoint: Option<String>,
    /// Optional environment-variable name holding the circuit token.
    pub source_token_env: Option<String>,
}

/// Validate and resolve one journal from a normalized topology map.
///
/// # Errors
/// Fails closed on invalid ids, missing journals, malformed role fields,
/// missing sources, self-follow, or a replica cycle.
pub fn resolve_journal(
    primary_journal: &str,
    journals: &BTreeMap<String, JournalEntry>,
    selected: Option<&str>,
) -> Result<ResolvedJournal, TopologyError> {
    let primary = JournalId::new(primary_journal)?;
    let primary_entry = journals
        .get(primary.as_str())
        .ok_or_else(|| TopologyError::MissingJournal(primary.to_string()))?;
    if primary_entry.role != JournalRole::Canonical {
        return Err(TopologyError::PrimaryNotCanonical(primary.to_string()));
    }
    validate_all(journals)?;
    let id = JournalId::new(selected.unwrap_or(primary.as_str()))?;
    let entry = journals
        .get(id.as_str())
        .ok_or_else(|| TopologyError::MissingJournal(id.to_string()))?;
    Ok(ResolvedJournal {
        id,
        role: entry.role,
        store_path: entry.store_path.clone(),
        source_journal: entry
            .source_journal
            .as_deref()
            .map(JournalId::new)
            .transpose()?,
        replica_mode: entry.replica_mode,
        source_endpoint: entry.source_endpoint.clone(),
        source_token_env: entry.source_token_env.clone(),
    })
}

fn validate_all(journals: &BTreeMap<String, JournalEntry>) -> Result<(), TopologyError> {
    if journals.is_empty() {
        return Err(TopologyError::Empty);
    }
    let mut paths = BTreeMap::<&str, String>::new();
    for (id, entry) in journals {
        let id = JournalId::new(id)?;
        if entry.store_path.trim().is_empty() {
            return Err(TopologyError::EmptyStorePath(id.to_string()));
        }
        if let Some(first) = paths.insert(entry.store_path.as_str(), id.to_string()) {
            return Err(TopologyError::DuplicateStorePath {
                first,
                second: id.to_string(),
                path: entry.store_path.clone(),
            });
        }
        match entry.role {
            JournalRole::Canonical => {
                if entry.source_journal.is_some()
                    || entry.replica_mode.is_some()
                    || entry.source_endpoint.is_some()
                    || entry.source_token_env.is_some()
                {
                    return Err(TopologyError::CanonicalHasReplicaFields(id.to_string()));
                }
            }
            JournalRole::Replica => {
                let source = entry
                    .source_journal
                    .as_deref()
                    .ok_or_else(|| TopologyError::ReplicaMissingSource(id.to_string()))?;
                let source = JournalId::new(source)?;
                if source == id {
                    return Err(TopologyError::ReplicaCycle(id.to_string()));
                }
                if !journals.contains_key(source.as_str()) {
                    return Err(TopologyError::MissingSource {
                        replica: id.to_string(),
                        source_journal: source.to_string(),
                    });
                }
                if entry.replica_mode.is_none() {
                    return Err(TopologyError::ReplicaMissingMode(id.to_string()));
                }
                match (&entry.source_endpoint, &entry.source_token_env) {
                    (Some(endpoint), Some(token_env))
                        if crate::compat::netbat::private_socket_addr(endpoint).is_ok()
                            && valid_env_name(token_env) => {}
                    (None, None) => {}
                    _ => return Err(TopologyError::InvalidRemoteSource(id.to_string())),
                }
                if entry.source_endpoint.is_some()
                    && entry.replica_mode == Some(ReplicaMode::ExactFork)
                {
                    return Err(TopologyError::RemoteExactFork(id.to_string()));
                }
            }
        }
    }
    for id in journals.keys() {
        let mut seen = BTreeSet::new();
        let mut cursor = id.as_str();
        while let Some(source) = journals
            .get(cursor)
            .and_then(|entry| entry.source_journal.as_deref())
        {
            if !seen.insert(cursor.to_string()) {
                return Err(TopologyError::ReplicaCycle(id.clone()));
            }
            cursor = source;
        }
    }
    Ok(())
}

fn valid_env_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_topology_resolves_canonical_and_replica() {
        let mut journals = BTreeMap::new();
        journals.insert(
            "canonical".to_string(),
            JournalEntry::canonical(".texo/store"),
        );
        journals.insert(
            "codex".to_string(),
            JournalEntry::replica(
                ".texo/replicas/codex",
                "canonical",
                ReplicaMode::ImportedReadModel,
            ),
        );
        let canonical = resolve_journal("canonical", &journals, None).expect("canonical");
        assert_eq!(canonical.role, JournalRole::Canonical);
        let replica = resolve_journal("canonical", &journals, Some("codex")).expect("replica");
        assert_eq!(replica.role, JournalRole::Replica);
        assert_eq!(
            replica.source_journal.as_ref().map(JournalId::as_str),
            Some("canonical")
        );
    }

    #[test]
    fn topology_rejects_cycles_and_replica_primary() {
        let mut journals = BTreeMap::new();
        journals.insert(
            "left".to_string(),
            JournalEntry::replica("left", "right", ReplicaMode::ImportedReadModel),
        );
        journals.insert(
            "right".to_string(),
            JournalEntry::replica("right", "left", ReplicaMode::ImportedReadModel),
        );
        assert!(matches!(
            resolve_journal("left", &journals, None),
            Err(TopologyError::PrimaryNotCanonical(_))
        ));
        journals.insert(
            "canonical".to_string(),
            JournalEntry::canonical("canonical"),
        );
        assert!(matches!(
            resolve_journal("canonical", &journals, None),
            Err(TopologyError::ReplicaCycle(_))
        ));
    }

    #[test]
    fn topology_rejects_two_ids_for_one_store_path() {
        let journals = BTreeMap::from([
            (
                "canonical".to_string(),
                JournalEntry::canonical(".texo/store"),
            ),
            (
                "replica".to_string(),
                JournalEntry::replica(".texo/store", "canonical", ReplicaMode::ImportedReadModel),
            ),
        ]);
        assert!(matches!(
            resolve_journal("canonical", &journals, None),
            Err(TopologyError::DuplicateStorePath { .. })
        ));
    }

    #[test]
    fn remote_topology_refuses_public_plaintext_endpoint() {
        let journals = BTreeMap::from([
            (
                "canonical".to_string(),
                JournalEntry::canonical(".texo/store"),
            ),
            (
                "remote".to_string(),
                JournalEntry::remote_replica(
                    ".texo/replicas/remote",
                    "canonical",
                    "8.8.8.8:9000",
                    "TEXO_REPLICA_TOKEN",
                ),
            ),
        ]);
        assert!(matches!(
            resolve_journal("canonical", &journals, Some("remote")),
            Err(TopologyError::InvalidRemoteSource(id)) if id == "remote"
        ));
    }
}
