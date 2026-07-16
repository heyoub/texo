//! Durable relation-campaign authority gate integration tests.

mod support;
#[path = "support/model.rs"]
mod model_support;

use model_support::write_model_capable_config;
use serde_json::{json, Value};
use support::{TestResult, TestWorkspace, OBSERVED_AT_MS};
use tempfile::TempDir;
use texo::events::coordinate::{
    coordinate_for_relation_campaign, coordinate_for_relation_pair, entity_for_relation_campaign,
};
use texo::events::ids::{relation_pair_id, ClaimId, WorkspaceId};
use texo::events::payloads::{RelationCampaignCheckpointV1, RelationJudgedV1};
use texo::host::TexoHost;
use texo::relate::settlement::{CampaignPhase, SettledRelation};

fn model_capable_workspace() -> TestResult<TestWorkspace> {
    let dir = TempDir::new()?;
    write_model_capable_config(dir.path())?;
    let mut host = TexoHost::open(dir.path(), "demo", OBSERVED_AT_MS)?;
    let _initialized = host.invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))?;
    Ok(TestWorkspace { dir, host })
}

fn event_kinds(workspace: &TestWorkspace) -> Vec<String> {
    let scope = texo::events::coordinate::scope_for_workspace(workspace.host.workspace_id());
    workspace
        .host
        .store()
        .by_scope(&scope)
        .into_iter()
        .map(|entry| entry.event_kind().to_string())
        .collect()
}

fn domain_event_count(workspace: &TestWorkspace) -> usize {
    event_kinds(workspace).len()
}

fn latest_checkpoint(workspace: &TestWorkspace) -> TestResult<RelationCampaignCheckpointV1> {
    let store = workspace.host.store();
    let entity = entity_for_relation_campaign(workspace.host.workspace_id());
    let entry = store
        .by_entity(&entity)
        .into_iter()
        .last()
        .ok_or("relation campaign checkpoint is absent")?;
    let raw = store.read_raw(entry.event_id())?;
    Ok(batpak::encoding::from_bytes(&raw.event.payload)?)
}

fn append_phase(
    workspace: &TestWorkspace,
    checkpoint: &RelationCampaignCheckpointV1,
    phase: CampaignPhase,
    observed_at_ms: u64,
) -> TestResult {
    let mut checkpoint = checkpoint.clone();
    checkpoint.phase = phase;
    checkpoint.observed_at_ms = observed_at_ms;
    let coordinate = coordinate_for_relation_campaign(workspace.host.workspace_id())?;
    let _receipt = workspace
        .host
        .store()
        .append_typed(&coordinate, &checkpoint)?;
    Ok(())
}

fn append_judgment(workspace: &TestWorkspace, older: &ClaimId, newer: &ClaimId) -> TestResult {
    let workspace_id = WorkspaceId::try_from(workspace.host.workspace_id())?;
    let pair_id = relation_pair_id(&workspace_id, older, newer);
    let coordinate = coordinate_for_relation_pair(workspace_id.as_str(), pair_id.as_str())?;
    let payload = RelationJudgedV1 {
        workspace_id,
        older_claim: older.clone(),
        newer_claim: newer.clone(),
        relation: SettledRelation::Unrelated,
        score_ppm: 900_000,
        judge_fingerprint: "test:relation-campaign-gate".to_string(),
        cache_key_hex: "test-cache-key".to_string(),
        observed_at_ms: OBSERVED_AT_MS + 3,
    };
    let _receipt = workspace.host.store().append_typed(&coordinate, &payload)?;
    Ok(())
}

fn strict_compile(workspace: &mut TestWorkspace) -> Result<Value, texo::error::TexoError> {
    workspace.invoke(
        "texo.compile.run",
        &json!({
            "out_dir": "strict-public",
            "observed_at_ms": OBSERVED_AT_MS + 10
        }),
    )
}

fn strict_context(workspace: &mut TestWorkspace) -> Result<Value, texo::error::TexoError> {
    workspace.invoke(
        "texo.context.agent",
        &json!({
            "subject": null,
            "include_stale": true
        }),
    )
}

fn workspace_status(workspace: &mut TestWorkspace) -> Result<Value, texo::error::TexoError> {
    workspace.invoke("texo.workspace.status", &json!({"snapshot": null}))
}

fn assert_settlement_refusal(error: &texo::error::TexoError) {
    assert_eq!(error.code(), "op.runtime");
    assert!(
        error.to_string().contains("strict settlement refused"),
        "unexpected settlement refusal: {error}"
    );
}

fn claim_rows(workspace: &mut TestWorkspace) -> TestResult<Vec<Value>> {
    let listed = workspace.invoke("texo.claims.list", &json!({"subject": null}))?;
    Ok(listed["claims"].as_array().cloned().unwrap_or_default())
}

#[test]
fn relate_requires_model_capability_before_handler_execution() -> TestResult {
    let mut workspace = TestWorkspace::new()?;
    let before = domain_event_count(&workspace);
    let error = workspace
        .invoke(
            "texo.relate.run",
            &json!({
                "observed_at_ms": OBSERVED_AT_MS + 1,
                "max_candidate_pairs": 1
            }),
        )
        .expect_err("relate without model capability must be denied");
    assert_eq!(error.code(), "op.denied");
    assert!(error.to_string().contains("texo.cap.model"));
    assert_eq!(domain_event_count(&workspace), before);
    Ok(())
}

#[test]
fn strict_outputs_require_latest_exact_complete_campaign() -> TestResult {
    let mut workspace = model_capable_workspace()?;
    let initialized_events = domain_event_count(&workspace);
    let initialized_kinds = event_kinds(&workspace);

    let absent = strict_compile(&mut workspace).expect_err("absent checkpoint must block compile");
    assert_settlement_refusal(&absent);
    assert_eq!(
        domain_event_count(&workspace),
        initialized_events,
        "event kinds changed from {initialized_kinds:?} to {:?}",
        event_kinds(&workspace)
    );
    assert!(!workspace.dir.path().join("strict-public").exists());
    assert_eq!(
        workspace_status(&mut workspace)?["settlement_complete"],
        false
    );

    let relate = workspace.invoke(
        "texo.relate.run",
        &json!({
            "observed_at_ms": OBSERVED_AT_MS + 1,
            "max_candidate_pairs": 1
        }),
    )?;
    assert_eq!(relate["outcome"], "complete");
    let complete = latest_checkpoint(&workspace)?;

    append_phase(
        &workspace,
        &complete,
        CampaignPhase::Partial {
            next_candidate_cursor: 7,
        },
        OBSERVED_AT_MS + 2,
    )?;
    let partial_events = domain_event_count(&workspace);
    let context_error = strict_context(&mut workspace).expect_err("partial must block context");
    assert_settlement_refusal(&context_error);
    let compile_error = strict_compile(&mut workspace).expect_err("partial must block compile");
    assert_settlement_refusal(&compile_error);
    assert_eq!(domain_event_count(&workspace), partial_events);
    assert!(!workspace.dir.path().join("strict-public").exists());
    assert_eq!(
        workspace_status(&mut workspace)?["settlement_complete"],
        false
    );

    append_phase(
        &workspace,
        &complete,
        CampaignPhase::Complete,
        OBSERVED_AT_MS + 3,
    )?;
    assert_eq!(
        workspace_status(&mut workspace)?["settlement_complete"],
        true
    );
    let _context = strict_context(&mut workspace)?;
    let compile = strict_compile(&mut workspace)?;
    assert!(compile["files"]
        .as_array()
        .is_some_and(|files| !files.is_empty()));
    assert!(workspace
        .dir
        .path()
        .join("strict-public/onboarding.generated.md")
        .exists());

    let before_noop = domain_event_count(&workspace);
    let noop = workspace.invoke(
        "texo.relate.run",
        &json!({
            "observed_at_ms": OBSERVED_AT_MS + 20,
            "max_candidate_pairs": 1
        }),
    )?;
    assert_eq!(noop["outcome"], "complete");
    assert_eq!(domain_event_count(&workspace), before_noop);
    Ok(())
}

#[test]
fn a_new_claim_invalidates_completion_after_it_becomes_noncurrent() -> TestResult {
    let mut workspace = model_capable_workspace()?;
    workspace.write("docs/current.md", "Deployments happen on Tuesday.\n")?;
    let _ingest = workspace.invoke(
        "texo.ingest.run",
        &json!({
            "path": "docs/current.md",
            "dry_run": false,
            "observed_at_ms": OBSERVED_AT_MS + 1
        }),
    )?;
    let original_claim = claim_rows(&mut workspace)?
        .into_iter()
        .next()
        .and_then(|claim| claim["claim_id"].as_str().map(str::to_string))
        .ok_or("original claim is absent")?;
    let relate = workspace.invoke(
        "texo.relate.run",
        &json!({
            "observed_at_ms": OBSERVED_AT_MS + 2,
            "max_candidate_pairs": 1
        }),
    )?;
    assert_eq!(relate["outcome"], "complete");
    let _context = strict_context(&mut workspace)?;

    workspace.write("docs/retired.md", "Deployments used to happen on Friday.\n")?;
    let _ingest = workspace.invoke(
        "texo.ingest.run",
        &json!({
            "path": "docs/retired.md",
            "dry_run": false,
            "observed_at_ms": OBSERVED_AT_MS + 3
        }),
    )?;
    let added_claim = claim_rows(&mut workspace)?
        .into_iter()
        .filter_map(|claim| claim["claim_id"].as_str().map(str::to_string))
        .find(|claim_id| claim_id != &original_claim)
        .ok_or("added claim is absent")?;
    let _supersede = workspace.invoke(
        "texo.claim.supersede",
        &json!({
            "old": added_claim.as_str(),
            "new": original_claim.as_str(),
            "reason": "historical claim retired",
            "decided_by": "human",
            "observed_at_ms": OBSERVED_AT_MS + 4
        }),
    )?;
    let rows = claim_rows(&mut workspace)?;
    assert!(rows
        .iter()
        .any(|claim| { claim["claim_id"] == added_claim && claim["status"] == "superseded" }));

    let error = strict_context(&mut workspace)
        .expect_err("new ineligible claim must invalidate prior completion");
    assert_settlement_refusal(&error);
    Ok(())
}

#[test]
fn rejudge_rejects_a_pair_that_left_the_current_claim_set() -> TestResult {
    let mut workspace = model_capable_workspace()?;
    workspace.write("docs/older.md", "Decision: deployments happen on Friday.\n")?;
    workspace.write(
        "docs/newer.md",
        "Decision: deployments now happen on Tuesday.\n",
    )?;
    for (offset, path) in ["docs/older.md", "docs/newer.md"].into_iter().enumerate() {
        let _ingest = workspace.invoke(
            "texo.ingest.run",
            &json!({
                "path": path,
                "dry_run": false,
                "observed_at_ms": OBSERVED_AT_MS + offset as u64
            }),
        )?;
    }
    let claims = claim_rows(&mut workspace)?;
    let [older_row, newer_row] = claims.as_slice() else {
        return Err(format!("expected two claims, got {}", claims.len()).into());
    };
    let older = ClaimId::try_from(older_row["claim_id"].as_str().ok_or("older claim id")?)?;
    let newer = ClaimId::try_from(newer_row["claim_id"].as_str().ok_or("newer claim id")?)?;
    append_judgment(&workspace, &older, &newer)?;
    let _supersede = workspace.invoke(
        "texo.claim.supersede",
        &json!({
            "old": older,
            "new": newer,
            "reason": "new schedule is authoritative",
            "decided_by": "human",
            "observed_at_ms": OBSERVED_AT_MS + 4
        }),
    )?;
    let before = domain_event_count(&workspace);
    let error = workspace
        .invoke(
            "texo.relate.run",
            &json!({
                "observed_at_ms": OBSERVED_AT_MS + 5,
                "max_candidate_pairs": 1,
                "rejudge_pair": [older, newer]
            }),
        )
        .expect_err("noncurrent rejudge pair must be rejected");
    assert_eq!(error.code(), "op.input");
    assert!(error.to_string().contains("no longer present"));
    assert_eq!(domain_event_count(&workspace), before);
    Ok(())
}
