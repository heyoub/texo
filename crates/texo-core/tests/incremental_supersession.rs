//! PROVES: F1 multi-session incremental supersession across separate ingest calls.

mod support;

use std::path::Path;

use support::{repo_sample_sources, setup_demo_journal, temp_workspace};
use texo_core::{ingest_sources, open_journal, ClaimStatus, IngestMode, FIXTURE_OBSERVED_AT_MS};

/// Copy a single named sample source into `dest_dir`.
fn copy_one_source(name: &str, dest_dir: &Path) {
    std::fs::create_dir_all(dest_dir).expect("mkdir session dir");
    let src = repo_sample_sources().join(name);
    std::fs::copy(&src, dest_dir.join(name)).expect("copy single source");
}

fn ingest_dir(root: &Path, input: &Path) {
    let journal = open_journal(root).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    ingest_sources(
        journal.handle(),
        journal.config(),
        &workspace,
        input,
        IngestMode::Commit,
        FIXTURE_OBSERVED_AT_MS,
        root,
    )
    .expect("ingest");
    journal.close().expect("close");
}

#[test]
fn second_session_supersedes_first_session_claim() {
    let dir = temp_workspace();
    setup_demo_journal(dir.path());

    // Session 1: ingest the old process (Friday deploy) in its own batch.
    let session1 = dir.path().join("session1");
    copy_one_source("old_process.md", &session1);
    ingest_dir(dir.path(), &session1);

    // Capture the Friday deploy claim id committed in session 1.
    let journal = open_journal(dir.path()).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    let after_session1 = journal.replay(&workspace).expect("replay");
    journal.close().expect("close");

    let friday_claim_id = after_session1
        .state
        .claims
        .values()
        .find(|c| c.subject_hint == "deploy-process" && c.normalized_text.contains("friday"))
        .map(|c| c.claim_id.clone())
        .expect("session 1 must record a Friday deploy claim");

    // The Friday deploy claim is the only deploy-process claim in session 1, so
    // it must not be superseded yet.
    assert!(
        !after_session1
            .state
            .superseded
            .contains_key(&friday_claim_id),
        "Friday claim must not be superseded after session 1 alone"
    );
    assert_eq!(
        after_session1
            .state
            .claims
            .get(&friday_claim_id)
            .expect("friday claim present after session 1")
            .status,
        ClaimStatus::Current,
        "Friday claim must be Current after session 1"
    );

    // Session 2: ingest the meeting notes (deploys moved to Tuesday) as a
    // SEPARATE ingest call. This exercises active_claims_for_supersession,
    // loading the session-1 Friday claim as a historical candidate.
    let session2 = dir.path().join("session2");
    copy_one_source("meeting_notes.md", &session2);
    ingest_dir(dir.path(), &session2);

    let journal = open_journal(dir.path()).expect("open");
    let after_session2 = journal.replay(&workspace).expect("replay");
    journal.close().expect("close");

    // The session-1 Friday claim must now be superseded by a session-2 claim.
    let superseded = after_session2
        .state
        .superseded
        .get(&friday_claim_id)
        .expect("F1: session-1 Friday claim must be superseded by a session-2 claim");
    assert_ne!(
        superseded.new_claim_id, friday_claim_id,
        "supersession edge must point to a different (new) claim"
    );

    let friday_view = after_session2
        .state
        .claims
        .get(&friday_claim_id)
        .expect("friday claim still present after session 2");
    assert_eq!(
        friday_view.status,
        ClaimStatus::Superseded,
        "F1: cross-session supersession must flip old claim to Superseded"
    );
    assert_eq!(
        friday_view.superseded_by.as_ref().map(ToString::to_string),
        Some(superseded.new_claim_id.to_string()),
        "F1: superseded_by must match the supersession edge target"
    );
}
