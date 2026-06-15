//! Golden snapshot for stale_onboarding staleness report.

mod support;

use insta::assert_json_snapshot;
use serde::Serialize;
use support::{
    copy_sample_sources, ingest_sample_sources, setup_demo_journal, stale_onboarding_report,
    temp_workspace,
};

#[derive(Serialize)]
struct StalenessSummary {
    checked_path: String,
    diagnostic_count: usize,
    lines: Vec<u32>,
    files: Vec<String>,
}

#[test]
fn staleness_stale_onboarding_snapshot() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    setup_demo_journal(dir.path());
    ingest_sample_sources(dir.path());

    let report = stale_onboarding_report(dir.path());
    let summary = StalenessSummary {
        checked_path: report.checked_path.clone(),
        diagnostic_count: report.diagnostics.len(),
        lines: report.diagnostics.iter().map(|d| d.line_start).collect(),
        files: report.diagnostics.iter().map(|d| d.file.clone()).collect(),
    };
    assert_json_snapshot!("staleness_stale_onboarding", summary);
}
