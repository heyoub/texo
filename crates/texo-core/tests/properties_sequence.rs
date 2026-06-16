//! PROVES: INV-SEQUENCE-MONOTONIC + INV-CONFIDENCE-BOUNDED — LocalSequence /
//! ReplayFrontier merge to the maximum (never regress), and ConfidencePpm only
//! accepts 0..=MAX. These are the numeric typestate invariants that keep the
//! replay frontier from going backwards and confidence from going out of range.

mod common;

use common::proptest::config;
use proptest::prelude::*;

use texo_core::types::{ConfidencePpm, LocalSequence, ObservedAtMs, ReplayFrontier};

proptest! {
    #![proptest_config(config())]

    // ROUND-TRIP: LocalSequence::new(n).get() == n for every u64.
    #[test]
    fn local_sequence_round_trips(n in any::<u64>()) {
        prop_assert_eq!(LocalSequence::new(n).get(), n);
    }

    // MAX IS THE MAXIMUM: merging two sequences yields exactly the larger raw
    // value, and the operation is commutative. Guards the replay frontier merge.
    #[test]
    fn local_sequence_max_is_maximum(a in any::<u64>(), b in any::<u64>()) {
        let la = LocalSequence::new(a);
        let lb = LocalSequence::new(b);
        let merged = la.max(lb);
        prop_assert_eq!(merged.get(), a.max(b));
        // Commutative: order of merge must not change the result.
        prop_assert_eq!(la.max(lb), lb.max(la));
        // Idempotent: merging with itself is identity.
        prop_assert_eq!(la.max(la), la);
    }

    // MONOTONICITY: advancing a frontier never decreases it, and the result is
    // exactly max(current, incoming). A regressing frontier would re-process or
    // skip events on replay.
    #[test]
    fn frontier_advance_is_monotonic(start in any::<u64>(), incoming in any::<u64>()) {
        let mut frontier = ReplayFrontier::new(LocalSequence::new(start));
        frontier.advance(LocalSequence::new(incoming));
        let after = frontier.sequence().get();
        prop_assert!(after >= start, "frontier regressed: {after} < {start}");
        prop_assert_eq!(after, start.max(incoming));
    }

    // FOLDED ADVANCE: advancing through a stream of sequences leaves the frontier
    // at the running maximum, never below any seen value. Order-independent.
    #[test]
    fn frontier_advance_reaches_running_max(seq in prop::collection::vec(any::<u64>(), 0..32)) {
        let mut frontier = ReplayFrontier::ZERO;
        for &s in &seq {
            frontier.advance(LocalSequence::new(s));
        }
        let expected = seq.iter().copied().max().unwrap_or(0);
        prop_assert_eq!(frontier.sequence().get(), expected);
        // Every observed sequence is <= the final frontier.
        for &s in &seq {
            prop_assert!(s <= frontier.sequence().get());
        }
    }

    // ZERO is the additive identity for the frontier merge: advancing ZERO by n
    // gives n; advancing n by 0 leaves n.
    #[test]
    fn frontier_zero_identity(n in any::<u64>()) {
        let mut from_zero = ReplayFrontier::ZERO;
        from_zero.advance(LocalSequence::new(n));
        prop_assert_eq!(from_zero.sequence().get(), n);

        let mut by_zero = ReplayFrontier::new(LocalSequence::new(n));
        by_zero.advance(LocalSequence::new(0));
        prop_assert_eq!(by_zero.sequence().get(), n);
    }

    // CONFIDENCE BOUND (accept): every value in 0..=MAX is accepted and round
    // trips through get().
    #[test]
    fn confidence_accepts_in_range(v in 0u32..=ConfidencePpm::MAX) {
        let ppm = ConfidencePpm::new(v).expect("in-range ppm must be accepted");
        prop_assert_eq!(ppm.get(), v);
    }

    // CONFIDENCE BOUND (reject): every value strictly above MAX is rejected with
    // InvalidConfidence carrying the offending value. A value silently clamped
    // here would corrupt confidence comparisons downstream.
    #[test]
    fn confidence_rejects_out_of_range(over in (ConfidencePpm::MAX + 1)..=u32::MAX) {
        match ConfidencePpm::new(over) {
            Ok(p) => prop_assert!(false, "out-of-range ppm accepted: {}", p.get()),
            Err(e) => prop_assert_eq!(e.0, over),
        }
    }

    // OBSERVED-AT round trips: the timestamp newtype preserves its raw value.
    #[test]
    fn observed_at_round_trips(ms in any::<u64>()) {
        prop_assert_eq!(ObservedAtMs::new(ms).get(), ms);
    }
}
