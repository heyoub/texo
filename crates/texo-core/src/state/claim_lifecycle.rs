//! Claim lifecycle transition laws.

use crate::events::payloads::ClaimSuperseded;
use crate::types::status::ClaimStatus;

/// Illegal claim status transition.
///
/// The lifecycle is monotone toward `Superseded`: once superseded, a claim never
/// re-becomes current (the supersession edge is terminal), and `Recorded` always
/// produces a fresh `Current` claim under a new id rather than reviving an old
/// one. There is therefore no "superseded back to current" transition to reject,
/// so no such variant exists — the only illegal transition the lifecycle can
/// surface is a malformed supersession payload (see [`validate_supersession`]).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TransitionError {
    /// Source status that cannot transition.
    #[error("invalid transition from {from:?}")]
    InvalidFrom {
        /// Status that blocked the transition.
        from: ClaimStatus,
    },
}

/// Lifecycle events that mutate claim status during replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimLifecycleEvent {
    /// A new claim was recorded.
    Recorded,
    /// A claim was superseded.
    Superseded,
    /// A claim entered an open conflict.
    OpenConflict,
}

/// Apply a lifecycle event to a claim status.
pub fn transition(
    current: ClaimStatus,
    event: ClaimLifecycleEvent,
) -> Result<ClaimStatus, TransitionError> {
    match event {
        ClaimLifecycleEvent::Recorded => Ok(initial_claim_status()),
        ClaimLifecycleEvent::Superseded => apply_supersession(current),
        ClaimLifecycleEvent::OpenConflict => Ok(apply_open_conflict(current)),
    }
}

/// Apply supersession to a claim's status.
pub fn apply_supersession(status: ClaimStatus) -> Result<ClaimStatus, TransitionError> {
    match status {
        ClaimStatus::Current | ClaimStatus::Conflicting | ClaimStatus::Unknown => {
            Ok(ClaimStatus::Superseded)
        }
        ClaimStatus::Superseded => Ok(ClaimStatus::Superseded),
    }
}

/// Initial status when a claim is first recorded.
pub fn initial_claim_status() -> ClaimStatus {
    ClaimStatus::Current
}

/// Mark claim as conflicting.
pub fn apply_open_conflict(status: ClaimStatus) -> ClaimStatus {
    match status {
        ClaimStatus::Superseded => status,
        _ => ClaimStatus::Conflicting,
    }
}

/// Validate supersession payload references.
pub fn validate_supersession(payload: &ClaimSuperseded) -> Result<(), TransitionError> {
    if payload.old_claim_id == payload.new_claim_id {
        return Err(TransitionError::InvalidFrom {
            from: ClaimStatus::Unknown,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superseded_stays_superseded() {
        assert_eq!(
            transition(ClaimStatus::Superseded, ClaimLifecycleEvent::Superseded)
                .expect("transition"),
            ClaimStatus::Superseded
        );
    }

    #[test]
    fn current_becomes_superseded() {
        assert_eq!(
            transition(ClaimStatus::Current, ClaimLifecycleEvent::Superseded).expect("transition"),
            ClaimStatus::Superseded
        );
    }

    #[test]
    fn recorded_is_current() {
        assert_eq!(
            transition(ClaimStatus::Unknown, ClaimLifecycleEvent::Recorded).expect("transition"),
            ClaimStatus::Current
        );
    }

    #[test]
    fn recorded_resets_a_superseded_status_to_current() {
        // `Recorded` is total and id-fresh: even applied over a `Superseded`
        // status it yields `Current`, NOT an error. This is exactly why the old
        // `SupersededToCurrent` variant was dead — the lifecycle never rejects a
        // record. If someone reintroduced a guard returning an error here, this
        // assertion would fail.
        assert_eq!(
            transition(ClaimStatus::Superseded, ClaimLifecycleEvent::Recorded).expect("transition"),
            ClaimStatus::Current
        );
    }

    #[test]
    fn open_conflict_marks_current_as_conflicting() {
        assert_eq!(
            transition(ClaimStatus::Current, ClaimLifecycleEvent::OpenConflict)
                .expect("transition"),
            ClaimStatus::Conflicting
        );
    }

    #[test]
    fn open_conflict_does_not_revive_superseded() {
        // A superseded claim stays superseded even if a conflict is opened: the
        // terminal supersession status wins over conflicting.
        assert_eq!(
            transition(ClaimStatus::Superseded, ClaimLifecycleEvent::OpenConflict)
                .expect("transition"),
            ClaimStatus::Superseded
        );
        assert_eq!(
            apply_open_conflict(ClaimStatus::Superseded),
            ClaimStatus::Superseded
        );
    }

    #[test]
    fn conflicting_and_unknown_supersede_to_superseded() {
        assert_eq!(
            apply_supersession(ClaimStatus::Conflicting).expect("supersede"),
            ClaimStatus::Superseded
        );
        assert_eq!(
            apply_supersession(ClaimStatus::Unknown).expect("supersede"),
            ClaimStatus::Superseded
        );
    }

    #[test]
    fn validate_supersession_rejects_self_reference() {
        let payload = ClaimSuperseded {
            old_claim_id: "claim_aaaaaaaaaaaa".to_string(),
            new_claim_id: "claim_aaaaaaaaaaaa".to_string(),
            workspace_id: "demo".to_string(),
            reason: "self".to_string(),
            decided_by: "test".to_string(),
            observed_at_ms: 1,
        };
        assert_eq!(
            validate_supersession(&payload),
            Err(TransitionError::InvalidFrom {
                from: ClaimStatus::Unknown
            })
        );
    }

    #[test]
    fn validate_supersession_accepts_distinct_ids() {
        let payload = ClaimSuperseded {
            old_claim_id: "claim_aaaaaaaaaaaa".to_string(),
            new_claim_id: "claim_bbbbbbbbbbbb".to_string(),
            workspace_id: "demo".to_string(),
            reason: "replaced".to_string(),
            decided_by: "test".to_string(),
            observed_at_ms: 1,
        };
        assert_eq!(validate_supersession(&payload), Ok(()));
    }
}
