//! PROVES: INV-AGENT-CONTEXT-FRONTIER

mod support;

use support::{copy_sample_sources, ingest_sample_sources, setup_demo_journal, temp_workspace};
use texo_core::build_agent_context;

#[test]
fn agent_context_includes_frontier_and_current_claims() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    setup_demo_journal(dir.path());
    ingest_sample_sources(dir.path());

    let journal = texo_core::open_journal(dir.path()).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    let replayed = journal.replay(&workspace).expect("replay");
    let context = build_agent_context(&replayed.state, workspace.as_str(), None);
    journal.close().expect("close");

    assert!(context.replayed_through_sequence > 0);
    assert_eq!(context.freshness.kind, "batpak-local-frontier");
    assert!(
        !context.claims.is_empty(),
        "AGENT CONTEXT VIOLATED: must include current claims"
    );
    for claim in &context.claims {
        assert!(!claim.source.path.is_empty());
        assert!(claim.receipt.sequence > 0);
    }
}
