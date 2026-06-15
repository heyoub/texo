//! PROVES: INV-REPLAY-CURRENT-UNIQUE, INV-REPLAY-SUPERSESSION

mod support;

use support::{copy_sample_sources, ingest_sample_sources, setup_demo_journal, temp_workspace};
use texo_core::ClaimStatus;

#[test]
fn supersession_marks_old_deploy_claim_superseded() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    setup_demo_journal(dir.path());
    ingest_sample_sources(dir.path());

    let journal = texo_core::open_journal(dir.path()).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    let replayed = journal.replay(&workspace).expect("replay");
    journal.close().expect("close");

    let friday_claims: Vec<_> = replayed
        .state
        .claims
        .values()
        .filter(|c| c.normalized_text.contains("friday") && c.subject_hint == "deploy-process")
        .collect();

    assert!(
        friday_claims
            .iter()
            .any(|c| c.status == ClaimStatus::Superseded),
        "REPLAY TRUTH VIOLATED: Friday deploy claim must be superseded"
    );

    let tuesday_current = replayed.state.current_claims(Some("deploy-process"));
    assert!(
        tuesday_current
            .iter()
            .any(|c| c.normalized_text.contains("tuesday")),
        "REPLAY TRUTH VIOLATED: Tuesday deploy claim must remain current"
    );
}
