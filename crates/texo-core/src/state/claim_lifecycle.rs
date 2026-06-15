//! Claim lifecycle transition laws.

use crate::events::payloads::ClaimSuperseded;
use crate::types::status::ClaimStatus;

/// Illegal claim status transition.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TransitionError {
    /// Cannot transition from superseded back to current without a new claim id.
    #[error("superseded claim cannot become current")]
    SupersededToCurrent,
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
}
