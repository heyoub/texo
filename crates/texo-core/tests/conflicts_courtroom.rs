//! PROVES: INV-CONFLICT-SEMANTICS

mod support;

use support::{copy_sample_sources, ingest_sample_sources, setup_demo_journal, temp_workspace};
use texo_core::detect_conflicts;

#[test]
fn full_ingest_has_no_open_conflicts_after_supersession() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    setup_demo_journal(dir.path());
    ingest_sample_sources(dir.path());

    let journal = texo_core::open_journal(dir.path()).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    let replayed = journal.replay(&workspace).expect("replay");
    journal.close().expect("close");

    let report = detect_conflicts(&replayed.state, workspace.as_str());
    assert!(
        report.conflicts.is_empty(),
        "CONFLICT SEMANTICS VIOLATED: superseded deploy claims must not appear as conflicts"
    );
}
