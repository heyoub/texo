//! Claim and conflict transition machine helpers.

use batpak::typestate::Transition;
use serde::{Deserialize, Serialize};

use crate::events::payloads::{
    ClaimRecordedV2, ClaimSupersededV2, ConflictOpenedV2, ConflictResolvedV2,
};

mod claim_markers {
    batpak::define_state_machine!(
        claim_seal,
        ClaimPhase {
            Unrecorded,
            Current,
            Superseded
        }
    );
}

mod conflict_markers {
    batpak::define_state_machine!(
        conflict_seal,
        ConflictPhase {
            Unopened,
            Open,
            Resolved,
            Ignored
        }
    );
}

/// Claim phase marker trait.
pub trait ClaimPhase: claim_markers::ClaimPhase {}

impl<T: claim_markers::ClaimPhase> ClaimPhase for T {}

/// Current claim phase marker.
pub type Current = claim_markers::Current;
/// Superseded claim phase marker.
pub type Superseded = claim_markers::Superseded;
/// Unrecorded claim phase marker.
pub type Unrecorded = claim_markers::Unrecorded;
/// Conflict phase marker trait.
pub trait ConflictPhase: conflict_markers::ConflictPhase {}

impl<T: conflict_markers::ConflictPhase> ConflictPhase for T {}

/// Ignored conflict phase marker.
pub type Ignored = conflict_markers::Ignored;
/// Open conflict phase marker.
pub type Open = conflict_markers::Open;
/// Resolved conflict phase marker.
pub type Resolved = conflict_markers::Resolved;
/// Unopened conflict phase marker.
pub type Unopened = conflict_markers::Unopened;

/// Claim state machine identifier.
pub const CLAIM_MACHINE: &str = "texo.claim.v2";
/// Legal claim state edges.
pub const CLAIM_EDGES: &[(u64, u64)] = &[(0, 1), (1, 2)];
/// Conflict state machine identifier.
pub const CONFLICT_MACHINE: &str = "texo.conflict.v2";
/// Legal conflict state edges.
pub const CONFLICT_EDGES: &[(u64, u64)] = &[(0, 1), (1, 2), (1, 3)];

/// Versioned transition record stored inside domain transition payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionRecordV1 {
    /// Transition record schema version.
    pub schema_version: u32,
    /// State machine identifier.
    pub machine: String,
    /// Previous state numeric code.
    pub previous_state: u64,
    /// Next state numeric code.
    pub next_state: u64,
    /// Deterministic transition identifier as BLAKE3 hex.
    pub transition_id_hex: String,
    /// Event coordinates or lane references that caused this transition.
    pub causes: Vec<TransitionCauseV1>,
}

/// Cause reference for a domain state transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionCauseV1 {
    /// `BatPak` DAG lane that carried the cause.
    pub lane: u32,
    /// Opaque event or coordinate key for the cause.
    pub key: String,
}

/// Build a transition record for a legal domain edge.
#[must_use]
pub fn transition_record(
    machine: &str,
    entity: &str,
    previous_state: u64,
    next_state: u64,
    causes: Vec<TransitionCauseV1>,
    observed_at_ms: u64,
) -> TransitionRecordV1 {
    TransitionRecordV1 {
        schema_version: 1,
        machine: machine.to_string(),
        previous_state,
        next_state,
        transition_id_hex: transition_id(
            machine,
            entity,
            previous_state,
            next_state,
            &causes,
            observed_at_ms,
        ),
        causes,
    }
}

/// Deterministically derive a transition id.
#[must_use]
pub fn transition_id(
    machine: &str,
    entity: &str,
    previous_state: u64,
    next_state: u64,
    causes: &[TransitionCauseV1],
    observed_at_ms: u64,
) -> String {
    let mut cause_keys = causes
        .iter()
        .map(|cause| cause.key.as_str())
        .collect::<Vec<_>>();
    cause_keys.sort_unstable();

    let mut hasher = blake3::Hasher::new();
    hasher.update(machine.as_bytes());
    hasher.update(entity.as_bytes());
    hasher.update(&previous_state.to_be_bytes());
    hasher.update(&next_state.to_be_bytes());
    for key in cause_keys {
        hasher.update(key.as_bytes());
    }
    hasher.update(&observed_at_ms.to_be_bytes());
    hasher.finalize().to_hex().to_string()
}

/// Construct the only exported claim-record transition.
#[must_use]
pub fn record_claim(payload: ClaimRecordedV2) -> Transition<Unrecorded, Current, ClaimRecordedV2> {
    Transition::from_payload(payload)
}

/// Construct the only exported claim-supersede transition.
#[must_use]
pub fn supersede_claim(
    payload: ClaimSupersededV2,
) -> Transition<Current, Superseded, ClaimSupersededV2> {
    Transition::from_payload(payload)
}

/// Construct the only exported conflict-open transition.
#[must_use]
pub fn open_conflict(payload: ConflictOpenedV2) -> Transition<Unopened, Open, ConflictOpenedV2> {
    Transition::from_payload(payload)
}

/// Construct the only exported conflict-resolve transition.
#[must_use]
pub fn resolve_conflict(
    payload: ConflictResolvedV2,
) -> Transition<Open, Resolved, ConflictResolvedV2> {
    Transition::from_payload(payload)
}

/// Construct the only exported conflict-ignore transition.
#[must_use]
pub fn ignore_conflict(
    payload: ConflictResolvedV2,
) -> Transition<Open, Ignored, ConflictResolvedV2> {
    Transition::from_payload(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_id_is_deterministic_and_cause_order_independent() {
        let causes = vec![
            TransitionCauseV1 {
                lane: 1,
                key: "b".to_string(),
            },
            TransitionCauseV1 {
                lane: 0,
                key: "a".to_string(),
            },
        ];
        let reversed = vec![
            TransitionCauseV1 {
                lane: 0,
                key: "a".to_string(),
            },
            TransitionCauseV1 {
                lane: 1,
                key: "b".to_string(),
            },
        ];

        let first = transition_id(CLAIM_MACHINE, "claim:one", 1, 2, &causes, 42);
        let second = transition_id(CLAIM_MACHINE, "claim:one", 1, 2, &reversed, 42);

        assert_eq!(first, second);
    }
}
