//! PROVES: INV-REPLAY-ERRORS (F3/F4 on the REAL store path) — error variants
//! that the in-memory reducer tests force directly must also surface when the
//! offending event is genuinely committed to a BatPak store and then replayed
//! through `replay_workspace`. This guards the wiring between BatPak decode and
//! the replay reducer, not just the reducer in isolation.

mod support;

use support::{setup_demo_journal, temp_workspace};
use texo_core::events::{ClaimConflictDetected, ClaimRecorded};
use texo_core::{open_journal, FIXTURE_OBSERVED_AT_MS};

const SOURCE_ID: &str = "src_abc123def456";

fn recorded(claim_id: &str) -> ClaimRecorded {
    ClaimRecorded {
        claim_id: claim_id.to_string(),
        workspace_id: "demo".to_string(),
        source_id: SOURCE_ID.to_string(),
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
        observed_at_ms: FIXTURE_OBSERVED_AT_MS,
    }
}

#[test]
fn replay_surfaces_invalid_conflict_status_from_real_store() {
    let dir = temp_workspace();
    let journal = setup_demo_journal(dir.path());
    let workspace = journal.config().workspace().expect("workspace");

    let a = "claim_aaaaaaaaaaaa";
    let b = "claim_bbbbbbbbbbbb";
    journal.handle().append_claim(&recorded(a)).expect("a");
    journal.handle().append_claim(&recorded(b)).expect("b");

    // Journal a conflict event carrying a status string that ConflictStatus
    // cannot parse. append_conflict does not validate the status, so it commits;
    // the error must instead surface loudly at replay time.
    journal
        .handle()
        .append_conflict(&ClaimConflictDetected {
            conflict_id: "conflict_aaaaaaaaaaaa".to_string(),
            workspace_id: "demo".to_string(),
            claim_a: a.to_string(),
            claim_b: b.to_string(),
            reason: "test".to_string(),
            status: "totally_invalid_status".to_string(),
            observed_at_ms: FIXTURE_OBSERVED_AT_MS,
        })
        .expect("append conflict with bad status commits");

    journal.close().expect("close");

    let journal = open_journal(dir.path()).expect("reopen");
    let result = journal.replay(&workspace);
    journal.close().expect("close");

    let err = result.expect_err("invalid conflict status must fail replay, not be swallowed");
    let msg = err.to_string();
    assert!(
        msg.contains("invalid conflict status") && msg.contains("totally_invalid_status"),
        "expected ReplayError::InvalidStatus surfaced through JournalError, got: {msg}"
    );
}

#[test]
fn replay_surfaces_missing_claim_from_real_store() {
    let dir = temp_workspace();
    let journal = setup_demo_journal(dir.path());
    let workspace = journal.config().workspace().expect("workspace");

    // Record only the NEW claim, then journal a supersession that references an
    // OLD claim that was never recorded. Replay must reject it as MissingClaim.
    let new_id = "claim_bbbbbbbbbbbb";
    let old_id = "claim_aaaaaaaaaaaa";
    journal
        .handle()
        .append_claim(&recorded(new_id))
        .expect("new");
    journal
        .handle()
        .append_superseded(&texo_core::events::ClaimSuperseded {
            old_claim_id: old_id.to_string(),
            new_claim_id: new_id.to_string(),
            workspace_id: "demo".to_string(),
            reason: "test".to_string(),
            decided_by: "test".to_string(),
            observed_at_ms: FIXTURE_OBSERVED_AT_MS,
        })
        .expect("append supersession commits");

    journal.close().expect("close");

    let journal = open_journal(dir.path()).expect("reopen");
    let result = journal.replay(&workspace);
    journal.close().expect("close");

    let err = result.expect_err("supersession of an unrecorded claim must fail replay");
    let msg = err.to_string();
    assert!(
        msg.contains("missing claim") && msg.contains(old_id),
        "expected ReplayError::MissingClaim({old_id}) surfaced through JournalError, got: {msg}"
    );
}
