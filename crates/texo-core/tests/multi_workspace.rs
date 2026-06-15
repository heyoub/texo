//! Multi-workspace isolation integration test.

mod support;

use support::{copy_sample_sources, temp_workspace};
use texo_core::{
    ingest_sources, init_workspace, open_journal_with, IngestMode, FIXTURE_OBSERVED_AT_MS,
};

#[test]
fn demo_and_staging_workspaces_are_isolated() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    init_workspace(dir.path(), "demo").expect("init demo");
    init_workspace(dir.path(), "staging").expect("init staging");

    let demo_journal = open_journal_with(dir.path(), Some("demo")).expect("open demo");
    let demo_ws = demo_journal.config().workspace().expect("demo ws");
    ingest_sources(
        demo_journal.handle(),
        demo_journal.config(),
        &demo_ws,
        &dir.path().join("sample_sources"),
        IngestMode::Commit,
        FIXTURE_OBSERVED_AT_MS,
        dir.path(),
    )
    .expect("ingest demo");
    demo_journal.close().expect("close demo");

    let staging_journal = open_journal_with(dir.path(), Some("staging")).expect("open staging");
    let staging_ws = staging_journal.config().workspace().expect("staging ws");
    let staging_replayed = staging_journal.replay(&staging_ws).expect("replay staging");
    staging_journal.close().expect("close staging");

    assert_eq!(
        staging_replayed.state.claims.len(),
        0,
        "staging workspace must stay empty when only demo was ingested"
    );

    let demo_journal = open_journal_with(dir.path(), Some("demo")).expect("reopen demo");
    let demo_replayed = demo_journal
        .replay(&demo_journal.config().workspace().expect("ws"))
        .expect("replay demo");
    demo_journal.close().expect("close demo");
    assert!(
        !demo_replayed.state.claims.is_empty(),
        "demo workspace must contain ingested claims"
    );
}
