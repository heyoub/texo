//! PROVES: INV-STALE-EXACT-LINE

mod support;

use support::{
    copy_sample_sources, ingest_sample_sources, setup_demo_journal, stale_onboarding_report,
    temp_workspace,
};

#[test]
fn stale_onboarding_flags_friday_deploy_line() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    setup_demo_journal(dir.path());
    ingest_sample_sources(dir.path());

    let report = stale_onboarding_report(dir.path());
    let deploy_diag = report
        .diagnostics
        .iter()
        .find(|d| d.file.contains("stale_onboarding.md") && d.line_start == 5)
        .expect("STALENESS VIOLATED: expected line 5 diagnostic for Friday deploy");

    assert!(
        deploy_diag.message.contains("superseded"),
        "STALENESS VIOLATED: message must mention supersession"
    );
    assert!(deploy_diag.superseded_by.is_some());
}
