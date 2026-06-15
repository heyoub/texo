//! Shared integration test helpers for real BatPak stores.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use texo_core::{
    check_staleness, ingest_sources, init_workspace, open_journal, ClaimStatus, IngestMode,
    IngestReport, Journal, Open, FIXTURE_OBSERVED_AT_MS,
};

/// Create a temp workspace with demo config and return root path.
pub fn temp_workspace() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// Initialize and open a journal in a temp directory.
pub fn setup_demo_journal(root: &Path) -> Journal<Open> {
    init_workspace(root, "demo").expect("init");
    open_journal(root).expect("open journal")
}

/// Ingest bundled sample sources and return the committed report.
pub fn ingest_sample_sources_report(root: &Path) -> IngestReport {
    let journal = open_journal(root).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    let committed = ingest_sources(
        journal.handle(),
        journal.config(),
        &workspace,
        &root.join("sample_sources"),
        IngestMode::Commit,
        FIXTURE_OBSERVED_AT_MS,
    )
    .expect("ingest");
    journal.close().expect("close");
    committed.into()
}

/// Ingest bundled sample sources from the repo.
pub fn ingest_sample_sources(root: &Path) {
    let journal = open_journal(root).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    ingest_sources(
        journal.handle(),
        journal.config(),
        &workspace,
        &root.join("sample_sources"),
        IngestMode::Commit,
        FIXTURE_OBSERVED_AT_MS,
    )
    .expect("ingest");
    journal.close().expect("close");
}

/// Path to sample sources in the repo during tests.
pub fn repo_sample_sources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sample_sources")
}

/// Copy sample sources into a workspace for hermetic tests.
pub fn copy_sample_sources(root: &Path) {
    let dest = root.join("sample_sources");
    std::fs::create_dir_all(&dest).expect("mkdir sample_sources");
    for entry in std::fs::read_dir(repo_sample_sources()).expect("read sample_sources") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        std::fs::copy(entry.path(), dest.join(name)).expect("copy sample");
    }
}

/// Run check_staleness against stale_onboarding.md and return report JSON value.
pub fn stale_onboarding_report(root: &Path) -> texo_core::StalenessReport {
    let journal = open_journal(root).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    let replayed = journal.replay(&workspace).expect("replay");
    let report = check_staleness(
        &replayed.state,
        workspace.as_str(),
        &root.join("sample_sources/stale_onboarding.md"),
        root,
    )
    .expect("check staleness");
    journal.close().expect("close");
    report
}

/// Assert a claim has expected status after replay.
pub fn assert_claim_status(root: &Path, claim_id: &str, expected: ClaimStatus) {
    let journal = open_journal(root).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    let replayed = journal.replay(&workspace).expect("replay");
    let claim = replayed
        .state
        .claims
        .get(claim_id)
        .unwrap_or_else(|| panic!("missing claim {claim_id}"));
    assert_eq!(
        claim.status, expected,
        "REPLAY TRUTH VIOLATED: claim {claim_id} status mismatch"
    );
    journal.close().expect("close");
}
