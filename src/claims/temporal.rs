//! Deterministic replay projection of frozen source-snapshot order.

use std::collections::BTreeMap;

use batpak::coordinate::Region;
use batpak::event::EventPayload;
use batpak::store::{Open, Store};

use crate::error::TexoError;
use crate::events::coordinate::scope_for_workspace;
use crate::events::payloads::SourceSnapshotRelationV1;
use crate::knowledge::{SourceSnapshotId, TemporalRelation};

const PAGE_LIMIT: usize = 256;

/// Snapshot-order facts reconstructed exclusively from durable journal events.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TemporalProjection {
    relations: BTreeMap<(String, String), TemporalRelation>,
}

impl TemporalProjection {
    /// Compare `left` with `right` in the Git source domain.
    ///
    /// Same snapshot identities are equal without requiring a journal edge.
    /// A recorded reverse edge is inverted. Missing evidence remains unknown.
    #[must_use]
    pub fn compare(&self, left: &SourceSnapshotId, right: &SourceSnapshotId) -> TemporalRelation {
        if left == right {
            return TemporalRelation::Same;
        }
        let left = left.to_string();
        let right = right.to_string();
        self.relations
            .get(&(left.clone(), right.clone()))
            .copied()
            .or_else(|| self.relations.get(&(right, left)).copied().map(invert))
            .unwrap_or(TemporalRelation::Unknown)
    }

    /// Iterate replayed directed relation facts in stable identity order.
    pub fn facts(&self) -> impl Iterator<Item = (&str, &str, TemporalRelation)> {
        self.relations
            .iter()
            .map(|((left, right), relation)| (left.as_str(), right.as_str(), *relation))
    }

    /// Insert a replayed first-write relation fact.
    fn insert(&mut self, relation: &SourceSnapshotRelationV1) {
        self.relations
            .entry((
                relation.left_snapshot_id.to_string(),
                relation.right_snapshot_id.to_string(),
            ))
            .or_insert(relation.relation);
    }
}

/// Assemble source-snapshot order through one durable frontier.
///
/// Replay performs no Git access. The first fact for a directed snapshot pair
/// wins deterministically if a malformed future journal contains duplicates.
///
/// # Errors
/// Returns a store or typed-decode error for unreadable journal source truth.
pub fn assemble_through(
    store: &Store<Open>,
    workspace_id: &str,
    frontier: u64,
) -> Result<TemporalProjection, TexoError> {
    let region = Region::scope(scope_for_workspace(workspace_id));
    let mut after = None;
    let mut projection = TemporalProjection::default();
    'pages: loop {
        let page = store.query_entries_after(&region, after, PAGE_LIMIT);
        if page.is_empty() {
            break;
        }
        for entry in &page {
            if entry.global_sequence() > frontier {
                break 'pages;
            }
            if entry.event_kind() != <SourceSnapshotRelationV1 as EventPayload>::KIND {
                continue;
            }
            let raw = store.read_raw(entry.event_id())?;
            let relation =
                batpak::encoding::from_bytes::<SourceSnapshotRelationV1>(&raw.event.payload)
                    .map_err(|error| TexoError::Decode {
                        entity: entry.coord().entity().to_string(),
                        detail: error.to_string(),
                    })?;
            projection.insert(&relation);
        }
        after = page.last().map(batpak::store::IndexEntry::global_sequence);
    }
    Ok(projection)
}

const fn invert(relation: TemporalRelation) -> TemporalRelation {
    match relation {
        TemporalRelation::Before => TemporalRelation::After,
        TemporalRelation::After => TemporalRelation::Before,
        TemporalRelation::Same => TemporalRelation::Same,
        TemporalRelation::Concurrent => TemporalRelation::Concurrent,
        TemporalRelation::Unknown => TemporalRelation::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_lookup_inverts_only_directional_relations() {
        let left = SourceSnapshotId::derive("left");
        let right = SourceSnapshotId::derive("right");
        let mut projection = TemporalProjection::default();
        projection.relations.insert(
            (left.to_string(), right.to_string()),
            TemporalRelation::Before,
        );

        assert_eq!(projection.compare(&left, &right), TemporalRelation::Before);
        assert_eq!(projection.compare(&right, &left), TemporalRelation::After);
        assert_eq!(projection.compare(&left, &left), TemporalRelation::Same);
        assert_eq!(
            projection.compare(&left, &SourceSnapshotId::derive("missing")),
            TemporalRelation::Unknown
        );
    }
}
