//! PROVES: INV-CONFLICT-COMMIT (conflicts/commit.rs) — `commit_conflicts` must
//! journal a real `ClaimConflictDetected` event against a live store for each
//! newly-detected open conflict, mark the participating claims `Conflicting` on
//! the next replay, and be idempotent (a second commit over a state that already
//! contains the conflict appends nothing).
//!
//! This file exercises `conflicts/commit.rs`, which was at 0% coverage: the
//! happy commit path, the journaled outcome (event + claim status), the
//! "already present -> skip" branch, and the empty-report (no-op) branch.

mod support;

use support::{setup_demo_journal, temp_workspace};
use texo_core::events::ClaimRecorded;
use texo_core::{
    commit_conflicts, detect_conflicts, open_journal, verify_journal_receipts, ClaimId,
    ClaimStatus, SourceId, FIXTURE_OBSERVED_AT_MS,
};

const SOURCE_ID: &str = "src_abc123def456";

/// Build a deploy-schedule claim whose subject/object give `detect_conflicts` a
/// contradiction signal (Friday vs Tuesday on the deploy-process subject).
fn deploy_claim(claim_id: &str, day: &str) -> ClaimRecorded {
    ClaimRecorded {
        claim_id: claim_id.to_string(),
        workspace_id: "demo".to_string(),
        source_id: SOURCE_ID.to_string(),
        source_path: "schedule.md".to_string(),
        line_start: 1,
        line_end: 1,
        text: format!("Deploy window is {day}."),
        normalized_text: format!("deploy window is {day}"),
        subject_hint: "deploy-process".to_string(),
        predicate_hint: "unknown".to_string(),
        object_hint: day.to_string(),
        confidence_ppm: 600_000,
        extractor_kind: "test".to_string(),
        observed_at_ms: FIXTURE_OBSERVED_AT_MS,
    }
}

#[test]
fn commit_conflicts_journals_detected_conflict_and_marks_claims() {
    let dir = temp_workspace();
    let journal = setup_demo_journal(dir.path());
    let workspace = journal.config().workspace().expect("workspace");

    // Two contradictory current claims under the same subject. Use the real
    // append path so the store holds genuine committed events.
    let a = "claim_aaaaaaaaaaaa";
    let b = "claim_bbbbbbbbbbbb";
    journal
        .handle()
        .append_claim(&deploy_claim(a, "friday"))
        .expect("append a");
    journal
        .handle()
        .append_claim(&deploy_claim(b, "tuesday"))
        .expect("append b");

    // Replay to a real ClaimState, confirm the conflict is detected but NOT yet
    // journaled.
    let replayed = journal.replay(&workspace).expect("replay");
    let report = detect_conflicts(&replayed.state, &workspace);
    assert_eq!(
        report.conflicts.len(),
        1,
        "fixture must produce exactly one detectable conflict"
    );
    assert!(
        replayed.state.conflicts.is_empty(),
        "no conflict event has been journaled yet"
    );

    // Commit the detected conflict against the live store.
    let committed = commit_conflicts(
        journal.handle(),
        &replayed.state,
        &workspace,
        FIXTURE_OBSERVED_AT_MS,
    )
    .expect("commit conflicts");
    assert_eq!(committed.len(), 1, "exactly one conflict must be committed");
    let committed_id = committed[0].conflict_id.clone();
    assert!(
        committed[0].sequence > 0,
        "committed conflict must carry a real append sequence"
    );

    journal.close().expect("close");

    // Reopen and replay: the journaled ClaimConflictDetected must now be present
    // and the participating claims must be marked Conflicting.
    let journal = open_journal(dir.path()).expect("reopen");
    let replayed = journal.replay(&workspace).expect("replay 2");

    assert!(
        replayed.state.conflicts.contains_key(&committed_id),
        "committed conflict must appear in replayed state"
    );
    let view = &replayed.state.conflicts[&committed_id];
    assert_eq!(view.status, texo_core::ConflictStatus::Open);

    // Receipt verification must accept a store that now contains a real
    // ClaimConflictDetected event: this drives the conflict arm of
    // `verify_journal_receipts`'s per-event receipt extraction.
    verify_journal_receipts(journal.handle().store(), &workspace)
        .expect("journal receipts including the conflict event must verify");

    for id in [a, b] {
        let claim_id = ClaimId::try_from(id).expect("claim id");
        let claim = &replayed.state.claims[&claim_id];
        assert_eq!(
            claim.status,
            ClaimStatus::Conflicting,
            "claim {id} must be Conflicting after the conflict is journaled"
        );
    }

    // Idempotence: committing again over a state that already contains the
    // conflict must journal nothing (the contains_key skip branch).
    let second = commit_conflicts(
        journal.handle(),
        &replayed.state,
        &workspace,
        FIXTURE_OBSERVED_AT_MS,
    )
    .expect("second commit");
    assert!(
        second.is_empty(),
        "re-committing an already-journaled conflict must be a no-op"
    );

    journal.close().expect("close");
}

#[test]
fn commit_conflicts_on_clean_state_is_a_noop() {
    // The empty-report branch: a workspace with no contradictory claims must
    // journal nothing and return an empty vector.
    let dir = temp_workspace();
    let journal = setup_demo_journal(dir.path());
    let workspace = journal.config().workspace().expect("workspace");

    journal
        .handle()
        .append_claim(&deploy_claim("claim_aaaaaaaaaaaa", "friday"))
        .expect("append one");

    let replayed = journal.replay(&workspace).expect("replay");
    let committed = commit_conflicts(
        journal.handle(),
        &replayed.state,
        &workspace,
        FIXTURE_OBSERVED_AT_MS,
    )
    .expect("commit");
    assert!(
        committed.is_empty(),
        "a single claim cannot conflict; commit must be a no-op"
    );

    journal.close().expect("close");

    // Sanity: ensure SourceId parses (keeps the import meaningful and asserts the
    // fixture source id is well-formed).
    assert!(SourceId::try_from(SOURCE_ID).is_ok());
}
