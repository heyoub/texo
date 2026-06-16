//! Replay reducer typestate.

use std::marker::PhantomData;

use crate::events::envelope::TexoEvent;
use crate::replay::apply::{apply_event, ReplayError};
use crate::replay::state::ClaimState;
use crate::types::sequence::{LocalSequence, ReplayFrontier};

/// Marker: replay not yet performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unreplayed;

/// Marker: replay complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Replayed;

/// Typestate replay folder.
pub struct ReplayReducer<State> {
    state: ClaimState,
    _marker: PhantomData<State>,
}

/// Result of a completed journal replay.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayedState {
    /// Projected claim state.
    pub state: ClaimState,
    /// Replay frontier.
    pub frontier: ReplayFrontier,
}

fn fold_events_internal(
    events: impl IntoIterator<Item = TexoEvent>,
) -> Result<ReplayedState, ReplayError> {
    let mut state = ClaimState::default();
    for event in events {
        apply_event(&mut state, &event)?;
    }
    state.rebuild_subject_index();
    Ok(ReplayedState {
        frontier: ReplayFrontier::new(LocalSequence::new(state.replayed_through_sequence)),
        state,
    })
}

impl ReplayedState {
    /// Fold events into replayed state.
    pub fn from_events(events: impl IntoIterator<Item = TexoEvent>) -> Result<Self, ReplayError> {
        fold_events_internal(events)
    }
}

impl ReplayReducer<Unreplayed> {
    /// Create an empty reducer.
    pub fn new() -> Self {
        Self {
            state: ClaimState::default(),
            _marker: PhantomData,
        }
    }

    /// Fold events in commit order.
    pub fn fold(
        mut self,
        events: impl IntoIterator<Item = TexoEvent>,
    ) -> Result<ReplayReducer<Replayed>, ReplayError> {
        for event in events {
            apply_event(&mut self.state, &event)?;
        }
        self.state.rebuild_subject_index();
        Ok(ReplayReducer {
            state: self.state,
            _marker: PhantomData,
        })
    }
}

impl Default for ReplayReducer<Unreplayed> {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayReducer<Replayed> {
    /// Extract state and frontier.
    pub fn into_state(self) -> ReplayedState {
        ReplayedState {
            frontier: ReplayFrontier::new(LocalSequence::new(self.state.replayed_through_sequence)),
            state: self.state,
        }
    }
}

/// Propagate replay errors when folding a slice of events.
pub fn fold_events(events: &[TexoEvent]) -> Result<ReplayedState, ReplayError> {
    fold_events_internal(events.iter().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::envelope::TexoEvent;
    use crate::events::payloads::ClaimRecorded;
    use crate::types::receipt::receipt_view;

    #[test]
    fn fold_propagates_invalid_id_error() {
        let receipt = receipt_view(1, 1, "ClaimRecorded", "workspace:demo", "claim:bad");
        let event = TexoEvent::ClaimRecorded {
            payload: ClaimRecorded {
                claim_id: "not_a_valid_claim_id".to_string(),
                workspace_id: "demo".to_string(),
                source_id: "src_abc123def456".to_string(),
                source_path: "x.md".to_string(),
                line_start: 1,
                line_end: 1,
                text: "x".to_string(),
                normalized_text: "x".to_string(),
                subject_hint: "s".to_string(),
                predicate_hint: "unknown".to_string(),
                object_hint: "x".to_string(),
                confidence_ppm: 500_000,
                extractor_kind: "test".to_string(),
                observed_at_ms: 1,
            },
            receipt,
        };
        let result = fold_events(&[event]);
        assert!(matches!(result, Err(ReplayError::InvalidId(_))));
    }

    fn valid_recorded(claim_id: &str, sequence: u64) -> TexoEvent {
        TexoEvent::ClaimRecorded {
            payload: ClaimRecorded {
                claim_id: claim_id.to_string(),
                workspace_id: "demo".to_string(),
                source_id: "src_abc123def456".to_string(),
                source_path: "x.md".to_string(),
                line_start: 1,
                line_end: 1,
                text: "x".to_string(),
                normalized_text: "x".to_string(),
                subject_hint: "s".to_string(),
                predicate_hint: "unknown".to_string(),
                object_hint: "x".to_string(),
                confidence_ppm: 500_000,
                extractor_kind: "test".to_string(),
                observed_at_ms: 1,
            },
            receipt: receipt_view(
                sequence.into(),
                sequence,
                "ClaimRecorded",
                "workspace:demo",
                claim_id,
            ),
        }
    }

    #[test]
    fn typestate_reducer_folds_and_reports_frontier() {
        // Exercise the typestate path (new -> fold -> into_state), not just the
        // free fold_events helper. The frontier must equal the max sequence seen.
        let events = [
            valid_recorded("claim_aaaaaaaaaaaa", 4),
            valid_recorded("claim_bbbbbbbbbbbb", 9),
        ];
        let replayed = ReplayReducer::new()
            .fold(events.iter().cloned())
            .expect("fold")
            .into_state();
        assert_eq!(replayed.state.claims.len(), 2);
        assert_eq!(replayed.frontier.sequence().get(), 9);
    }

    #[test]
    fn default_reducer_matches_new() {
        let events = [valid_recorded("claim_aaaaaaaaaaaa", 1)];
        let from_default = ReplayReducer::<Unreplayed>::default()
            .fold(events.iter().cloned())
            .expect("fold default")
            .into_state();
        let from_new = ReplayReducer::new()
            .fold(events.iter().cloned())
            .expect("fold new")
            .into_state();
        assert_eq!(from_default, from_new);
    }

    #[test]
    fn from_events_matches_typestate_path() {
        let events = [
            valid_recorded("claim_aaaaaaaaaaaa", 2),
            valid_recorded("claim_bbbbbbbbbbbb", 5),
        ];
        let via_helper = ReplayedState::from_events(events.iter().cloned()).expect("from_events");
        let via_typestate = ReplayReducer::new()
            .fold(events.iter().cloned())
            .expect("fold")
            .into_state();
        assert_eq!(via_helper, via_typestate);
    }

    #[test]
    fn typestate_fold_propagates_errors() {
        // The fold() path must surface the same error the free helper does.
        let receipt = receipt_view(1, 1, "ClaimRecorded", "workspace:demo", "claim:bad");
        let event = TexoEvent::ClaimRecorded {
            payload: ClaimRecorded {
                claim_id: "not_valid".to_string(),
                workspace_id: "demo".to_string(),
                source_id: "src_abc123def456".to_string(),
                source_path: "x.md".to_string(),
                line_start: 1,
                line_end: 1,
                text: "x".to_string(),
                normalized_text: "x".to_string(),
                subject_hint: "s".to_string(),
                predicate_hint: "unknown".to_string(),
                object_hint: "x".to_string(),
                confidence_ppm: 500_000,
                extractor_kind: "test".to_string(),
                observed_at_ms: 1,
            },
            receipt,
        };
        let result = ReplayReducer::new().fold([event]);
        assert!(matches!(result, Err(ReplayError::InvalidId(_))));
    }
}
