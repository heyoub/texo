//! `BatPak` coordinate builders for texo v2 entity streams.

use batpak::coordinate::{Coordinate, CoordinateError};

/// Coordinate scope for a workspace.
pub fn scope_for_workspace(workspace_id: &str) -> String {
    format!("workspace:{workspace_id}")
}

/// Entity string for a claim stream.
pub fn entity_for_claim(claim_id: &str) -> String {
    format!("claim:{claim_id}")
}

/// Entity string for a conflict stream.
pub fn entity_for_conflict(conflict_id: &str) -> String {
    format!("conflict:{conflict_id}")
}

/// Entity string for a source stream.
pub fn entity_for_source(source_id: &str) -> String {
    format!("source:{source_id}")
}

/// Entity string for the onboarding projection stream.
pub fn entity_for_onboarding_projection() -> String {
    "projection:onboarding".to_string()
}

/// Entity string for workspace metadata.
pub fn entity_for_workspace_meta(workspace_id: &str) -> String {
    format!("workspace-meta:{workspace_id}")
}

/// Entity string for a session stream.
pub fn entity_for_session(session_id: &str) -> String {
    format!("session:{session_id}")
}

/// Entity string for one provider-neutral logical relation pair.
pub fn entity_for_relation_pair(pair_id: &str) -> String {
    format!("relation:{pair_id}")
}

/// Entity string for one frozen source snapshot.
pub fn entity_for_source_snapshot(snapshot_id: &str) -> String {
    format!("source-snapshot:{snapshot_id}")
}

/// Entity string for one evidence occurrence.
pub fn entity_for_evidence(occurrence_id: &str) -> String {
    format!("evidence:{occurrence_id}")
}

/// Entity string for one disposable code-index registration.
pub fn entity_for_code_index(index_id: &str) -> String {
    format!("code-index:{index_id}")
}

/// Entity string for one directional frozen-snapshot comparison.
pub fn entity_for_source_relation(left_snapshot_id: &str, right_snapshot_id: &str) -> String {
    format!("source-relation:{left_snapshot_id}:{right_snapshot_id}")
}

/// Build a claim coordinate.
///
/// # Errors
/// Returns [`CoordinateError`] if the generated coordinate violates `BatPak`
/// coordinate validation.
pub fn coordinate_for_claim(
    workspace_id: &str,
    claim_id: &str,
) -> Result<Coordinate, CoordinateError> {
    Coordinate::new(
        entity_for_claim(claim_id),
        scope_for_workspace(workspace_id),
    )
}

/// Build a conflict coordinate.
///
/// # Errors
/// Returns [`CoordinateError`] if the generated coordinate violates `BatPak`
/// coordinate validation.
pub fn coordinate_for_conflict(
    workspace_id: &str,
    conflict_id: &str,
) -> Result<Coordinate, CoordinateError> {
    Coordinate::new(
        entity_for_conflict(conflict_id),
        scope_for_workspace(workspace_id),
    )
}

/// Build a source coordinate.
///
/// # Errors
/// Returns [`CoordinateError`] if the generated coordinate violates `BatPak`
/// coordinate validation.
pub fn coordinate_for_source(
    workspace_id: &str,
    source_id: &str,
) -> Result<Coordinate, CoordinateError> {
    Coordinate::new(
        entity_for_source(source_id),
        scope_for_workspace(workspace_id),
    )
}

/// Build the onboarding projection coordinate.
///
/// # Errors
/// Returns [`CoordinateError`] if the generated coordinate violates `BatPak`
/// coordinate validation.
pub fn coordinate_for_onboarding_projection(
    workspace_id: &str,
) -> Result<Coordinate, CoordinateError> {
    Coordinate::new(
        entity_for_onboarding_projection(),
        scope_for_workspace(workspace_id),
    )
}

/// Build a workspace metadata coordinate.
///
/// # Errors
/// Returns [`CoordinateError`] if the generated coordinate violates `BatPak`
/// coordinate validation.
pub fn coordinate_for_workspace_meta(workspace_id: &str) -> Result<Coordinate, CoordinateError> {
    Coordinate::new(
        entity_for_workspace_meta(workspace_id),
        scope_for_workspace(workspace_id),
    )
}

/// Build a session coordinate.
///
/// # Errors
/// Returns [`CoordinateError`] if the generated coordinate violates `BatPak`
/// coordinate validation.
pub fn coordinate_for_session(
    workspace_id: &str,
    session_id: &str,
) -> Result<Coordinate, CoordinateError> {
    Coordinate::new(
        entity_for_session(session_id),
        scope_for_workspace(workspace_id),
    )
}

/// Build a logical relation-pair coordinate.
///
/// # Errors
/// Returns [`CoordinateError`] if the generated coordinate is invalid.
pub fn coordinate_for_relation_pair(
    workspace_id: &str,
    pair_id: &str,
) -> Result<Coordinate, CoordinateError> {
    Coordinate::new(
        entity_for_relation_pair(pair_id),
        scope_for_workspace(workspace_id),
    )
}

/// Build a frozen source-snapshot coordinate.
///
/// # Errors
/// Returns [`CoordinateError`] if the generated coordinate is invalid.
pub fn coordinate_for_source_snapshot(
    workspace_id: &str,
    snapshot_id: &str,
) -> Result<Coordinate, CoordinateError> {
    Coordinate::new(
        entity_for_source_snapshot(snapshot_id),
        scope_for_workspace(workspace_id),
    )
}

/// Build an evidence-occurrence coordinate.
///
/// # Errors
/// Returns [`CoordinateError`] if the generated coordinate is invalid.
pub fn coordinate_for_evidence(
    workspace_id: &str,
    occurrence_id: &str,
) -> Result<Coordinate, CoordinateError> {
    Coordinate::new(
        entity_for_evidence(occurrence_id),
        scope_for_workspace(workspace_id),
    )
}

/// Build a code-index registration coordinate.
///
/// # Errors
/// Returns [`CoordinateError`] if the generated coordinate is invalid.
pub fn coordinate_for_code_index(
    workspace_id: &str,
    index_id: &str,
) -> Result<Coordinate, CoordinateError> {
    Coordinate::new(
        entity_for_code_index(index_id),
        scope_for_workspace(workspace_id),
    )
}

/// Build a frozen source-snapshot relation coordinate.
///
/// # Errors
/// Returns [`CoordinateError`] if the generated coordinate is invalid.
pub fn coordinate_for_source_relation(
    workspace_id: &str,
    left_snapshot_id: &str,
    right_snapshot_id: &str,
) -> Result<Coordinate, CoordinateError> {
    Coordinate::new(
        entity_for_source_relation(left_snapshot_id, right_snapshot_id),
        scope_for_workspace(workspace_id),
    )
}

/// Deterministically map a session id to a non-zero `BatPak` lane.
pub fn session_lane(session_id: &str) -> u32 {
    let hash = blake3::hash(session_id.as_bytes());
    let bytes = hash.as_bytes();
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) | 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_and_entity_builders_use_the_expected_prefixes() {
        assert_eq!(scope_for_workspace("demo"), "workspace:demo");
        assert_eq!(entity_for_claim("claim_abc"), "claim:claim_abc");
        assert_eq!(entity_for_conflict("conflict_abc"), "conflict:conflict_abc");
        assert_eq!(entity_for_source("src_abc"), "source:src_abc");
        assert_eq!(entity_for_onboarding_projection(), "projection:onboarding");
        assert_eq!(entity_for_workspace_meta("demo"), "workspace-meta:demo");
        assert_eq!(entity_for_session("s1"), "session:s1");
        assert_eq!(
            entity_for_source_snapshot("snapshot_abc"),
            "source-snapshot:snapshot_abc"
        );
        assert_eq!(entity_for_evidence("evidence_abc"), "evidence:evidence_abc");
        assert_eq!(
            entity_for_code_index("code_index_abc"),
            "code-index:code_index_abc"
        );
        assert_eq!(
            entity_for_source_relation("snapshot_a", "snapshot_b"),
            "source-relation:snapshot_a:snapshot_b"
        );
    }

    #[test]
    fn coordinate_builders_produce_valid_batpak_coordinates() {
        let coords = [
            coordinate_for_claim("demo", "claim_abc").expect("claim coordinate"),
            coordinate_for_conflict("demo", "conflict_abc").expect("conflict coordinate"),
            coordinate_for_source("demo", "src_abc").expect("source coordinate"),
            coordinate_for_onboarding_projection("demo").expect("projection coordinate"),
            coordinate_for_workspace_meta("demo").expect("workspace metadata coordinate"),
            coordinate_for_session("demo", "session_abc").expect("session coordinate"),
            coordinate_for_source_snapshot("demo", "snapshot_abc")
                .expect("source snapshot coordinate"),
            coordinate_for_evidence("demo", "evidence_abc").expect("evidence coordinate"),
            coordinate_for_code_index("demo", "code_index_abc").expect("code index coordinate"),
            coordinate_for_source_relation("demo", "snapshot_a", "snapshot_b")
                .expect("source relation coordinate"),
        ];

        for coord in coords {
            coord.validate().expect("coordinate validates");
            assert_eq!(coord.scope(), "workspace:demo");
        }
    }

    #[test]
    fn session_lane_is_deterministic_and_non_zero() {
        let first = session_lane("session_abc");
        let second = session_lane("session_abc");

        assert_eq!(first, second);
        assert_ne!(first, 0);
    }
}
