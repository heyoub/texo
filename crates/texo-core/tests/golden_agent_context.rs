//! Golden snapshot for agent context after demo ingest.

mod support;

use insta::assert_json_snapshot;
use serde::Serialize;
use support::{copy_sample_sources, ingest_sample_sources, setup_demo_journal, temp_workspace};
use texo_core::build_agent_context;

#[derive(Serialize)]
struct AgentContextGolden {
    workspace_id: String,
    replayed_through_sequence: u64,
    freshness_kind: String,
    claims: Vec<ClaimGolden>,
    stale: Vec<StaleGolden>,
}

#[derive(Serialize)]
struct ClaimGolden {
    claim_id: String,
    subject_hint: String,
    text: String,
    source_path: String,
    line_start: u32,
    sequence: u64,
    supersedes: Vec<String>,
}

#[derive(Serialize)]
struct StaleGolden {
    claim_id: String,
    superseded_by: String,
}

#[test]
fn agent_context_demo_snapshot() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    setup_demo_journal(dir.path());
    ingest_sample_sources(dir.path());

    let journal = texo_core::open_journal(dir.path()).expect("open");
    let workspace = journal.config().workspace().expect("workspace");
    let replayed = journal.replay(&workspace).expect("replay");
    let context = build_agent_context(&replayed.state, workspace.as_str(), None);
    journal.close().expect("close");

    let golden = AgentContextGolden {
        workspace_id: context.workspace_id,
        replayed_through_sequence: context.replayed_through_sequence,
        freshness_kind: context.freshness.kind,
        claims: context
            .claims
            .iter()
            .map(|c| ClaimGolden {
                claim_id: c.claim_id.to_string(),
                subject_hint: c.subject_hint.clone(),
                text: c.text.clone(),
                source_path: c.source.path.clone(),
                line_start: c.source.line_start,
                sequence: c.receipt.sequence,
                supersedes: c
                    .supersedes
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
            })
            .collect(),
        stale: context
            .stale_claims
            .iter()
            .map(|s| StaleGolden {
                claim_id: s.claim_id.to_string(),
                superseded_by: s.superseded_by.to_string(),
            })
            .collect(),
    };
    assert_json_snapshot!("agent_context_demo", golden);
}
