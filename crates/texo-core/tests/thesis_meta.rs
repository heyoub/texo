//! PROVES: THESIS-STALE-ONBOARDING
//! Given full sample_sources ingest, stale onboarding must expose superseded claims.

mod support;

use support::{
    copy_sample_sources, ingest_sample_sources, setup_demo_journal, stale_onboarding_report,
    temp_workspace,
};

#[test]
fn thesis_stale_onboarding_exposes_supersession() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    setup_demo_journal(dir.path());
    ingest_sample_sources(dir.path());

    let report = stale_onboarding_report(dir.path());
    assert!(
        !report.diagnostics.is_empty(),
        "THESIS FAILED: expected stale diagnostics in stale_onboarding.md"
    );

    let diag = &report.diagnostics[0];
    assert!(
        diag.superseded_by.is_some(),
        "THESIS FAILED: diagnostic must reference superseding claim"
    );
    assert!(
        diag.receipt.is_some() || diag.source.is_some(),
        "THESIS FAILED: diagnostic must include provenance"
    );
    assert!(report.replayed_through_sequence > 0);
}
