//! PROVES: INV-REPLAY-DETERMINISTIC + INV-REPLAY-IDEMPOTENT — folding the SAME
//! event sequence twice yields byte-identical ClaimState (determinism), and the
//! replay is a pure function of the event slice (no hidden global state). Also
//! checks the specified order-sensitivity: a ClaimRecorded must precede any
//! ClaimSuperseded that references it, otherwise replay errors loudly (it never
//! silently produces a different state).

mod common;

use std::collections::BTreeSet;

use common::proptest::config;
use proptest::prelude::*;

use texo_core::events::payloads::{ClaimRecorded, ClaimSuperseded};
use texo_core::events::TexoEvent;
use texo_core::replay::{fold_events, ReplayedState};
use texo_core::types::ids::{claim_id_from_parts, source_id_from_hash, SourceId};
use texo_core::types::receipt::receipt_view;
use texo_core::types::sequence::ConfidencePpm;

const SOURCE_BODY: &[u8] = b"deterministic fixture body";

fn source_id() -> SourceId {
    source_id_from_hash(&texo_core::types::ids::blake3_bytes_hex(SOURCE_BODY)).expect("source id")
}

/// Build a recorded-claim event whose id is derived deterministically from
/// (source, line, text) so distinct rows get distinct ids — matching production.
fn recorded(line: u32, text: &str, ppm: u32, sequence: u64) -> (String, TexoEvent) {
    let src = source_id();
    let claim_id = claim_id_from_parts(&src, line, text);
    let id_str = claim_id.to_string();
    // Clamp confidence into range so the payload is well-formed (the value range
    // is itself property-tested in properties_sequence.rs).
    let ppm = ConfidencePpm::new(ppm % (ConfidencePpm::MAX + 1))
        .expect("clamped ppm")
        .get();
    let event = TexoEvent::ClaimRecorded {
        payload: ClaimRecorded {
            claim_id: id_str.clone(),
            workspace_id: "demo".to_string(),
            source_id: src.to_string(),
            source_path: "doc.md".to_string(),
            line_start: line,
            line_end: line,
            text: text.to_string(),
            normalized_text: text.to_string(),
            subject_hint: "subject".to_string(),
            predicate_hint: "unknown".to_string(),
            object_hint: text.to_string(),
            confidence_ppm: ppm,
            extractor_kind: "test".to_string(),
            observed_at_ms: u64::from(line),
        },
        receipt: receipt_view(sequence.into(), sequence, "ClaimRecorded", "demo", &id_str),
    };
    (id_str, event)
}

fn supersede(old: &str, new: &str, sequence: u64) -> TexoEvent {
    TexoEvent::ClaimSuperseded {
        payload: ClaimSuperseded {
            old_claim_id: old.to_string(),
            new_claim_id: new.to_string(),
            workspace_id: "demo".to_string(),
            reason: "newer information".to_string(),
            decided_by: "tester".to_string(),
            observed_at_ms: 100,
        },
        receipt: receipt_view(sequence.into(), sequence, "ClaimSuperseded", "demo", old),
    }
}

/// A coherent event sequence: a set of distinct recorded claims, optionally
/// followed by one valid supersession (old != new, both recorded). Returns the
/// events plus the count of distinct recorded claim ids.
fn arb_event_sequence() -> impl Strategy<Value = (Vec<TexoEvent>, usize)> {
    (
        prop::collection::vec(("[a-z]{2,8}", 1u32..200u32, any::<u32>()), 1..8),
        any::<bool>(),
    )
        .prop_map(|(rows, do_supersede)| {
            let mut events = Vec::new();
            let mut ids: Vec<String> = Vec::new();
            let mut seen: BTreeSet<String> = BTreeSet::new();
            let mut seq = 1u64;
            for (text, line, ppm) in rows {
                let (id, ev) = recorded(line, &text, ppm, seq);
                seq += 1;
                if seen.insert(id.clone()) {
                    ids.push(id);
                    events.push(ev);
                }
            }
            let distinct = ids.len();
            if do_supersede && ids.len() >= 2 {
                events.push(supersede(&ids[0], &ids[1], seq));
            }
            (events, distinct)
        })
}

proptest! {
    #![proptest_config(config())]

    // DETERMINISM + PURITY: folding the same slice twice yields identical state.
    // If replay leaked global mutable state, the two folds would diverge.
    #[test]
    fn fold_is_deterministic((events, _distinct) in arb_event_sequence()) {
        let a = fold_events(&events).expect("fold a");
        let b = fold_events(&events).expect("fold b");
        prop_assert_eq!(&a.state, &b.state);
        prop_assert_eq!(a.frontier, b.frontier);
        // ReplayedState::from_events must agree with fold_events (same reducer).
        let c = ReplayedState::from_events(events.iter().cloned()).expect("from_events");
        prop_assert_eq!(&a.state, &c.state);
    }

    // FRONTIER reflects the max sequence seen, never below the last event's seq.
    #[test]
    fn fold_frontier_is_max_sequence((events, _distinct) in arb_event_sequence()) {
        let folded = fold_events(&events).expect("fold");
        let max_seq = events.iter().map(|e| e.sequence().get()).max().unwrap_or(0);
        prop_assert_eq!(folded.frontier.sequence().get(), max_seq);
    }

    // STATE INTEGRITY: every recorded claim id appears in the projected claims
    // map, and the claim count equals the number of distinct recorded ids. A
    // reducer that dropped or merged claims would fail this.
    #[test]
    fn fold_preserves_all_recorded_claims((events, distinct) in arb_event_sequence()) {
        let folded = fold_events(&events).expect("fold");
        prop_assert_eq!(folded.state.claims.len(), distinct);
        for event in &events {
            if let TexoEvent::ClaimRecorded { payload, .. } = event {
                let id = texo_core::types::ids::ClaimId::try_from(payload.claim_id.as_str())
                    .expect("claim id");
                prop_assert!(folded.state.claims.contains_key(&id));
            }
        }
    }

    // ORDER-SENSITIVITY (specified): a ClaimSuperseded BEFORE its referenced
    // ClaimRecorded must fail with a typed error, not silently succeed or panic.
    // This pins the documented commit-order requirement.
    #[test]
    fn supersession_before_record_errors(text_a in "[a-z]{2,8}", text_b in "[a-z]{2,8}") {
        prop_assume!(text_a != text_b);
        let (id_a, rec_a) = recorded(1, &text_a, 500_000, 1);
        let (id_b, rec_b) = recorded(2, &text_b, 500_000, 2);
        prop_assume!(id_a != id_b);

        // Supersession appears (seq 1) before either claim is recorded (seq 2,3).
        let out_of_order = vec![supersede(&id_a, &id_b, 1), rec_a, rec_b];
        let result = fold_events(&out_of_order);
        prop_assert!(result.is_err(), "out-of-order supersession must error, got Ok");

        // Sanity: the SAME events in valid order succeed (non-vacuous: proves the
        // failure above is about ORDER, not malformed events).
        let (id_a2, rec_a2) = recorded(1, &text_a, 500_000, 1);
        let (id_b2, rec_b2) = recorded(2, &text_b, 500_000, 2);
        let in_order = vec![rec_a2, rec_b2, supersede(&id_a2, &id_b2, 3)];
        prop_assert!(fold_events(&in_order).is_ok(), "in-order supersession must succeed");
    }
}
