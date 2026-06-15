//! PROVES: INV-COMPILE-JOURNALED

mod support;

use support::{copy_sample_sources, ingest_sample_sources, setup_demo_journal, temp_workspace};
use texo_core::{compile_out, open_journal, FIXTURE_OBSERVED_AT_MS};

#[test]
fn compile_appends_onboarding_compiled_event() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    setup_demo_journal(dir.path());
    ingest_sample_sources(dir.path());

    let out = dir.path().join("public");
    compile_out(dir.path(), &out, FIXTURE_OBSERVED_AT_MS, None).expect("compile");

    let journal = open_journal(dir.path()).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    let events = journal.handle().load_events(&workspace).expect("events");
    journal.close().expect("close");

    assert!(
        events.iter().any(|e| e.kind() == "OnboardingCompiled"),
        "COMPILE VIOLATED: compile must append OnboardingCompiled to journal"
    );
}
