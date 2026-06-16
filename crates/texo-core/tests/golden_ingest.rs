//! Golden snapshot for committed ingest receipts.

mod support;

use insta::assert_json_snapshot;
use serde::Serialize;
use support::{
    copy_sample_sources, ingest_sample_sources_report, setup_demo_journal, temp_workspace,
};

#[derive(Serialize)]
struct IngestGolden {
    workspace_id: String,
    sources_observed: usize,
    claims_recorded: usize,
    receipts: Vec<ReceiptGolden>,
}

#[derive(Serialize)]
struct ReceiptGolden {
    kind: String,
    sequence: u64,
    scope: String,
    entity_prefix: String,
}

#[test]
fn ingest_demo_snapshot() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    setup_demo_journal(dir.path());
    let report = ingest_sample_sources_report(dir.path());

    let golden = IngestGolden {
        workspace_id: report.workspace_id.to_string(),
        sources_observed: report.sources_observed,
        claims_recorded: report.claims_recorded,
        receipts: report
            .receipts
            .iter()
            .map(|r| ReceiptGolden {
                kind: r.kind.clone(),
                sequence: r.sequence.get(),
                scope: r.scope.clone(),
                entity_prefix: r.entity.split(':').next().unwrap_or("").to_string(),
            })
            .collect(),
    };
    assert_json_snapshot!("ingest_demo", golden);
}
