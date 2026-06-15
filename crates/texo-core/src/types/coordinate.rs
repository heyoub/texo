//! BatPak coordinate builders for texo entity streams.

/// Coordinate scope for a workspace.
pub fn scope_for_workspace(workspace_id: &str) -> String {
    format!("workspace:{workspace_id}")
}

/// Entity string for a source stream.
pub fn entity_for_source(source_id: &str) -> String {
    format!("source:{source_id}")
}

/// Entity string for a claim stream.
pub fn entity_for_claim(claim_id: &str) -> String {
    format!("claim:{claim_id}")
}

/// Entity string for a conflict stream.
pub fn entity_for_conflict(conflict_id: &str) -> String {
    format!("conflict:{conflict_id}")
}

/// Entity string for a projection stream.
pub fn entity_for_projection(name: &str) -> String {
    format!("projection:{name}")
}

/// Entity stream kind prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    /// Source document stream.
    Source,
    /// Claim stream.
    Claim,
    /// Conflict stream.
    Conflict,
    /// Projection stream.
    Projection,
}

impl EntityKind {
    /// Prefix string for this entity kind.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Claim => "claim",
            Self::Conflict => "conflict",
            Self::Projection => "projection",
        }
    }
}
