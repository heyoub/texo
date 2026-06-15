//! PROVES: INV-REPLAY-DETERMINISTIC

mod support;

use support::{copy_sample_sources, ingest_sample_sources, setup_demo_journal, temp_workspace};
use texo_core::{ingest_sources, open_journal, IngestMode, FIXTURE_OBSERVED_AT_MS};

#[test]
fn close_and_reopen_yields_identical_state() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    setup_demo_journal(dir.path());
    ingest_sample_sources(dir.path());

    let journal = texo_core::open_journal(dir.path()).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    let first = journal.replay(&workspace).expect("replay");
    journal.close().expect("close");

    let journal = texo_core::open_journal(dir.path()).expect("reopen");
    let second = journal.replay(&workspace).expect("replay");
    journal.close().expect("close");

    assert_eq!(first, second);
}

#[test]
fn second_ingest_of_unchanged_sources_is_idempotent() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    setup_demo_journal(dir.path());
    ingest_sample_sources(dir.path());

    let journal = open_journal(dir.path()).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    let second = ingest_sources(
        journal.handle(),
        journal.config(),
        &workspace,
        &dir.path().join("sample_sources"),
        IngestMode::Commit,
        FIXTURE_OBSERVED_AT_MS,
        dir.path(),
    )
    .expect("second ingest");
    journal.close().expect("close");

    assert_eq!(
        second.sources_observed, 0,
        "REPLAY TRUTH VIOLATED: unchanged sources must not re-ingest"
    );
}
