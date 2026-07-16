//! Journaled Git snapshot and claim-evidence integration.

#[path = "support/model.rs"]
mod model_support;

use std::path::Path;
use std::process::Command;

use batpak::coordinate::Region;
use batpak::event::{EventKind, EventPayload};
use batpak::id::EntityIdType;
use serde_json::json;
use tempfile::TempDir;
use texo::events::coordinate::scope_for_workspace;
use texo::events::payloads::{
    ClaimEvidenceLinkedV1, ClaimSupersededV2, CodeIndexRecordedV1, EvidenceOccurrenceRecordedV1,
    SourceSnapshotRecordedV1, SourceSnapshotRelationV1,
};
use texo::host::TexoHost;

use model_support::write_model_capable_config;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn git(root: &Path, args: &[&str]) -> TestResult {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

fn git_stdout(root: &Path, args: &[&str]) -> TestResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

fn initialized_repository() -> TestResult<(TempDir, TexoHost)> {
    let root = TempDir::new()?;
    git(root.path(), &["init", "-q"])?;
    git(root.path(), &["config", "user.name", "Texo Test"])?;
    git(
        root.path(),
        &["config", "user.email", "texo@example.invalid"],
    )?;
    std::fs::create_dir_all(root.path().join("docs"))?;
    std::fs::create_dir_all(root.path().join("src"))?;
    std::fs::write(
        root.path().join("docs/decision.md"),
        "Decision: deploys happen on Friday.\n",
    )?;
    std::fs::write(
        root.path().join("src/lib.rs"),
        "pub fn deploy() { helper(); }\nfn helper() {}\n",
    )?;
    git(root.path(), &["add", "docs/decision.md", "src/lib.rs"])?;
    git(root.path(), &["commit", "-qm", "initial"])?;

    write_model_capable_config(root.path())?;
    let mut host = TexoHost::open(root.path(), "demo", 1_700_000_000_000)?;
    let _ = host.invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))?;
    let _ = host.invoke_json(
        "texo.ingest.run",
        &json!({
            "path": "docs/decision.md",
            "dry_run": false,
            "strict": false,
            "observed_at_ms": 1_700_000_000_001_u64
        }),
    )?;
    Ok((root, host))
}

fn count_workspace_events(host: &TexoHost) -> usize {
    let region = Region::scope(scope_for_workspace(host.workspace_id()));
    let mut after = None;
    let mut count = 0;
    loop {
        let page = host.store().query_entries_after(&region, after, 256);
        if page.is_empty() {
            return count;
        }
        count += page.len();
        after = page.last().map(batpak::store::IndexEntry::global_sequence);
    }
}

fn count_event_kind(host: &TexoHost, kind: EventKind) -> usize {
    let region = Region::scope(scope_for_workspace(host.workspace_id()));
    host.store()
        .query_entries_after(&region, None, usize::MAX)
        .into_iter()
        .filter(|entry| entry.event_kind() == kind)
        .count()
}

struct ReplacementScenario {
    root: TempDir,
    host: TexoHost,
    superseded_before: usize,
}

fn concurrent_replacement_scenario() -> TestResult<ReplacementScenario> {
    let root = TempDir::new()?;
    git(root.path(), &["init", "-q"])?;
    git(root.path(), &["config", "user.name", "Texo Test"])?;
    git(
        root.path(),
        &["config", "user.email", "texo@example.invalid"],
    )?;
    std::fs::write(root.path().join("README.md"), "base\n")?;
    git(root.path(), &["add", "README.md"])?;
    git(root.path(), &["commit", "-qm", "base"])?;
    let base = git_stdout(root.path(), &["branch", "--show-current"])?;

    git(root.path(), &["checkout", "-qb", "left"])?;
    std::fs::create_dir_all(root.path().join("docs"))?;
    std::fs::write(
        root.path().join("docs/decision.md"),
        "Decision: the deploy target is Frankfurt.\n",
    )?;
    git(root.path(), &["add", "docs/decision.md"])?;
    git(root.path(), &["commit", "-qm", "Frankfurt target"])?;
    let mut host = TexoHost::open(root.path(), "demo", 1_700_000_100_000)?;
    host.invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))?;
    host.invoke_json(
        "texo.ingest.run",
        &json!({"path":"docs/decision.md","dry_run":false,"strict":false,"observed_at_ms":1_700_000_100_001_u64}),
    )?;
    host.invoke_json(
        "texo.knowledge.index",
        &json!({"observed_at_ms":1_700_000_100_002_u64}),
    )?;

    git(root.path(), &["checkout", "-q", &base])?;
    git(root.path(), &["checkout", "-qb", "right"])?;
    std::fs::create_dir_all(root.path().join("docs"))?;
    std::fs::write(
        root.path().join("docs/decision.md"),
        "Decision: the deploy target now is Singapore.\n",
    )?;
    git(root.path(), &["add", "docs/decision.md"])?;
    git(root.path(), &["commit", "-qm", "Singapore target"])?;
    let superseded_before = count_event_kind(&host, <ClaimSupersededV2 as EventPayload>::KIND);
    Ok(ReplacementScenario {
        root,
        host,
        superseded_before,
    })
}

fn assert_unknown_replacement_is_held(scenario: &mut ReplacementScenario) -> TestResult {
    let ingest = scenario.host.invoke_json(
        "texo.ingest.run",
        &json!({"path":"docs/decision.md","dry_run":false,"strict":false,"observed_at_ms":1_700_000_100_003_u64}),
    )?;
    assert_eq!(ingest["claims_superseded"], 0);
    assert_eq!(ingest["supersessions_held"], 1);
    assert_eq!(
        ingest["held_supersessions"][0]["reason"],
        "temporal_unknown"
    );
    assert_eq!(
        count_event_kind(&scenario.host, <ClaimSupersededV2 as EventPayload>::KIND),
        scenario.superseded_before
    );
    Ok(())
}

fn assert_concurrent_replacement_is_held(scenario: &mut ReplacementScenario) -> TestResult {
    let concurrent = scenario.host.invoke_json(
        "texo.knowledge.index",
        &json!({"observed_at_ms":1_700_000_100_004_u64}),
    )?;
    assert_eq!(concurrent["supersessions_applied"], 0);
    assert_eq!(concurrent["supersessions_held"], 1);
    assert_eq!(
        concurrent["held_supersessions"][0]["reason"],
        "temporal_concurrent"
    );
    assert_eq!(
        count_event_kind(&scenario.host, <ClaimSupersededV2 as EventPayload>::KIND),
        scenario.superseded_before
    );
    Ok(())
}

fn assert_descendant_replacement_resumes(scenario: &mut ReplacementScenario) -> TestResult {
    git(scenario.root.path(), &["checkout", "-q", "left"])?;
    std::fs::write(
        scenario.root.path().join("docs/decision.md"),
        "Decision: the deploy target now is Singapore.\n",
    )?;
    git(scenario.root.path(), &["add", "docs/decision.md"])?;
    git(
        scenario.root.path(),
        &["commit", "-qm", "move target after Frankfurt"],
    )?;
    let descendant = scenario.host.invoke_json(
        "texo.knowledge.index",
        &json!({"observed_at_ms":1_700_000_100_005_u64}),
    )?;
    assert_eq!(descendant["supersessions_applied"], 1);
    assert_eq!(descendant["supersessions_held"], 0);
    assert_eq!(
        count_event_kind(&scenario.host, <ClaimSupersededV2 as EventPayload>::KIND),
        scenario.superseded_before + 1
    );
    let claims = scenario
        .host
        .invoke_json("texo.claims.list", &json!({"subject":null,"snapshot":null}))?;
    assert_eq!(
        claims["claims"]
            .as_array()
            .ok_or("claims")?
            .iter()
            .filter(|claim| claim["status"] == "superseded")
            .count(),
        1
    );
    Ok(())
}

#[test]
fn index_journals_snapshot_evidence_links_and_causal_headers_once() -> TestResult {
    let (_root, mut host) = initialized_repository()?;
    let first = host.invoke_json(
        "texo.knowledge.index",
        &json!({"observed_at_ms": 1_700_000_000_002_u64}),
    )?;
    assert_eq!(first["already_indexed"], false);
    assert_eq!(first["evidence_recorded"], 1);
    assert_eq!(first["claims_linked"], 1);

    let status = host.invoke_json("texo.workspace.status", &json!({}))?;
    assert_eq!(
        status["snapshot"]["descriptor"]["source_snapshot_id"],
        first["snapshot_id"]
    );
    assert_ne!(status["coverage"]["analysis_quality"], "unavailable");

    let region = Region::scope(scope_for_workspace("demo"));
    let entries = host.store().query_entries_after(&region, None, 256);
    let kind_entry = |kind: EventKind| {
        entries
            .iter()
            .find(|entry| entry.event_kind() == kind)
            .ok_or("event kind")
    };
    let snapshot = kind_entry(<SourceSnapshotRecordedV1 as EventPayload>::KIND)?;
    let evidence = kind_entry(<EvidenceOccurrenceRecordedV1 as EventPayload>::KIND)?;
    let link = kind_entry(<ClaimEvidenceLinkedV1 as EventPayload>::KIND)?;
    assert_eq!(evidence.correlation_id(), snapshot.event_id().as_u128());
    assert_eq!(evidence.causation_id(), Some(snapshot.event_id().as_u128()));
    assert_eq!(link.correlation_id(), snapshot.event_id().as_u128());
    assert_eq!(link.causation_id(), Some(evidence.event_id().as_u128()));

    let count = count_workspace_events(&host);
    let second = host.invoke_json(
        "texo.knowledge.index",
        &json!({"observed_at_ms": 1_700_000_000_003_u64}),
    )?;
    assert_eq!(second["already_indexed"], true);
    assert_eq!(count_workspace_events(&host), count);
    Ok(())
}

#[test]
fn changed_worktree_never_links_old_claim_as_supporting_evidence() -> TestResult {
    let (root, mut host) = initialized_repository()?;
    std::fs::write(
        root.path().join("docs/decision.md"),
        "Decision: deploys happen on Tuesday.\n",
    )?;
    let output = host.invoke_json(
        "texo.knowledge.index",
        &json!({"observed_at_ms": 1_700_000_000_002_u64}),
    )?;
    assert_eq!(output["evidence_recorded"], 0);
    assert_eq!(output["claims_linked"], 0);
    assert!(output["coverage"]["gaps"]
        .as_array()
        .is_some_and(|gaps| gaps.iter().any(|gap| {
            gap["path"] == "docs/decision.md" && gap["kind"] == "analysis_incomplete"
        })));
    Ok(())
}

#[test]
fn explicit_replacement_waits_for_git_authority_then_resumes_without_model() -> TestResult {
    let mut scenario = concurrent_replacement_scenario()?;
    assert_unknown_replacement_is_held(&mut scenario)?;
    assert_concurrent_replacement_is_held(&mut scenario)?;
    assert_descendant_replacement_resumes(&mut scenario)
}

#[test]
fn explain_and_triangulate_join_exact_evidence_at_one_snapshot() -> TestResult {
    let (_root, mut host) = initialized_repository()?;
    let before = host.invoke_json("texo.workspace.status", &json!({}))?;
    let before_token = before["snapshot"]["token"]
        .as_str()
        .ok_or("snapshot token")?;
    let claims = host.invoke_json(
        "texo.claims.list",
        &json!({"subject": null, "snapshot": null}),
    )?;
    let claim_id = claims["claims"][0]["claim_id"].as_str().ok_or("claim id")?;

    let _ = host.invoke_json(
        "texo.knowledge.index",
        &json!({"observed_at_ms": 1_700_000_000_002_u64}),
    )?;
    let related = host.invoke_json(
        "texo.relate.run",
        &json!({
            "observed_at_ms": 1_700_000_000_003_u64,
            "max_candidate_pairs": 1
        }),
    )?;
    assert_eq!(related["outcome"], "complete");
    let explain = host.invoke_json(
        "texo.claim.explain",
        &json!({"claim_id": claim_id, "snapshot": null}),
    )?;
    assert_eq!(explain["answer_state"], "supported");
    assert_eq!(explain["evidence"][0]["claim_id"], claim_id);
    assert_eq!(
        explain["evidence"][0]["occurrence"]["excerpt"],
        "Decision: deploys happen on Friday."
    );
    assert_eq!(
        explain["evidence"][0]["occurrence"]["path"],
        "docs/decision.md"
    );

    let triangulated = host.invoke_json(
        "texo.knowledge.triangulate",
        &json!({
            "target": {
                "kind": "path",
                "path": "docs/decision.md",
                "line_start": 1,
                "line_end": 1
            },
            "snapshot": null
        }),
    )?;
    assert_eq!(triangulated["answer_state"], "supported");
    assert_eq!(triangulated["assertions"].as_array().map(Vec::len), Some(1));
    assert_eq!(triangulated["evidence"].as_array().map(Vec::len), Some(1));
    assert_eq!(triangulated["settlement_complete"], true);

    let historical = host.invoke_json(
        "texo.claim.explain",
        &json!({"claim_id": claim_id, "snapshot": before_token}),
    )?;
    assert_eq!(historical["answer_state"], "unverified");
    assert_eq!(historical["evidence"].as_array().map(Vec::len), Some(0));
    assert_eq!(
        historical["coverage"]["gaps"][0]["kind"],
        "source_snapshot_unavailable"
    );
    Ok(())
}

#[test]
fn symbol_and_unsafe_path_targets_fail_honestly() -> TestResult {
    let (_root, mut host) = initialized_repository()?;
    let symbol = host.invoke_json(
        "texo.knowledge.triangulate",
        &json!({
            "target": {"kind": "symbol", "symbol": "rust-analyzer cargo_crate::item"},
            "snapshot": null
        }),
    )?;
    assert_eq!(symbol["answer_state"], "unverified");
    assert!(symbol["uncertainty"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row == "code_index_unavailable")));

    let error = host
        .invoke_json(
            "texo.knowledge.triangulate",
            &json!({
                "target": {"kind": "path", "path": "../secret", "line_start": null, "line_end": null},
                "snapshot": null
            }),
        )
        .expect_err("parent traversal must fail closed");
    assert_eq!(error.code(), "op.input");
    Ok(())
}

fn assert_code_search(host: &mut TexoHost) -> TestResult {
    let search = host.invoke_json(
        "texo.knowledge.search",
        &json!({
            "query": "deploy",
            "subject": null,
            "status": null,
            "limit": 25,
            "cursor": null,
            "snapshot": null
        }),
    )?;
    assert_eq!(search["code_index_available"], true, "{search}");
    assert!(search["results"].as_array().is_some_and(|rows| {
        rows.iter()
            .any(|row| row["kind"] == "code" && row["occurrence"]["display_name"] == "deploy")
    }));
    let first_page = host.invoke_json(
        "texo.knowledge.search",
        &json!({
            "query": "deploy",
            "subject": null,
            "status": null,
            "limit": 1,
            "cursor": null,
            "snapshot": null
        }),
    )?;
    if let Some(cursor) = first_page["next_cursor"].as_str() {
        let error = host
            .invoke_json(
                "texo.knowledge.search",
                &json!({
                    "query": "helper",
                    "subject": null,
                    "status": null,
                    "limit": 1,
                    "cursor": cursor,
                    "snapshot": null
                }),
            )
            .expect_err("cursor must be bound to its original query and snapshot");
        assert_eq!(error.code(), "op.input");
    }
    Ok(())
}

#[test]
fn code_index_is_causally_bound_and_symbol_triangulation_degrades_on_deletion() -> TestResult {
    let (root, mut host) = initialized_repository()?;
    let source = host.invoke_json(
        "texo.knowledge.index",
        &json!({"observed_at_ms": 1_700_000_000_002_u64}),
    )?;
    let code = host.invoke_json(
        "texo.code.index.build",
        &json!({
            "snapshot_id": source["snapshot_id"],
            "scip_path": null,
            "observed_at_ms": 1_700_000_000_003_u64
        }),
    )?;
    assert_eq!(code["format"], "syntax");
    assert_eq!(code["coverage"]["analysis_quality"], "syntactic");
    assert_code_search(&mut host)?;

    let region = Region::scope(scope_for_workspace("demo"));
    let entries = host.store().query_entries_after(&region, None, 256);
    let snapshot = entries
        .iter()
        .find(|entry| entry.event_kind() == <SourceSnapshotRecordedV1 as EventPayload>::KIND)
        .ok_or("source snapshot event")?;
    let code_event = entries
        .iter()
        .find(|entry| entry.event_kind() == <CodeIndexRecordedV1 as EventPayload>::KIND)
        .ok_or("code index event")?;
    assert_eq!(code_event.correlation_id(), snapshot.event_id().as_u128());
    assert_eq!(
        code_event.causation_id(),
        Some(snapshot.event_id().as_u128())
    );

    let triangulated = host.invoke_json(
        "texo.knowledge.triangulate",
        &json!({
            "target": {"kind": "symbol", "symbol": "deploy"},
            "snapshot": null
        }),
    )?;
    assert_eq!(triangulated["answer_state"], "supported");
    assert!(triangulated["structural_evidence"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| {
            row["display_name"] == "deploy" && row["analysis_quality"] == "syntactic"
        })));

    std::fs::remove_file(
        root.path()
            .join(code["artifact_path"].as_str().ok_or("artifact")?),
    )?;
    let degraded = host.invoke_json(
        "texo.knowledge.triangulate",
        &json!({
            "target": {"kind": "symbol", "symbol": "deploy"},
            "snapshot": null
        }),
    )?;
    assert_eq!(degraded["answer_state"], "unverified");
    assert!(degraded["uncertainty"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row == "code_index_unavailable")));
    assert!(degraded["coverage"]["gaps"]
        .as_array()
        .is_some_and(|rows| rows
            .iter()
            .any(|row| row["kind"] == "code_index_unavailable")));
    Ok(())
}

#[test]
fn indexing_records_pairwise_git_dag_relations_for_replay() -> TestResult {
    let (root, mut host) = initialized_repository()?;
    let first = host.invoke_json(
        "texo.knowledge.index",
        &json!({"observed_at_ms": 1_700_000_000_002_u64}),
    )?;
    let first_commit = git_stdout(root.path(), &["rev-parse", "HEAD"])?;

    std::fs::write(
        root.path().join("src/lib.rs"),
        "pub fn deploy() { helper(); }\nfn helper() {}\nfn linear() {}\n",
    )?;
    git(root.path(), &["add", "src/lib.rs"])?;
    git(root.path(), &["commit", "-qm", "linear"])?;
    let second = host.invoke_json(
        "texo.knowledge.index",
        &json!({"observed_at_ms": 1_700_000_000_003_u64}),
    )?;
    assert_eq!(second["relations_recorded"], 1);

    git(
        root.path(),
        &["checkout", "-qb", "concurrent", &first_commit],
    )?;
    std::fs::write(root.path().join("src/branch.rs"), "pub fn branch() {}\n")?;
    git(root.path(), &["add", "src/branch.rs"])?;
    git(root.path(), &["commit", "-qm", "concurrent"])?;
    let third = host.invoke_json(
        "texo.knowledge.index",
        &json!({"observed_at_ms": 1_700_000_000_004_u64}),
    )?;
    assert_eq!(third["relations_recorded"], 2);

    let region = Region::scope(scope_for_workspace("demo"));
    let mut relations = Vec::new();
    for entry in host.store().query_entries_after(&region, None, 256) {
        if entry.event_kind() != <SourceSnapshotRelationV1 as EventPayload>::KIND {
            continue;
        }
        let raw = host.store().read_raw(entry.event_id())?;
        relations.push(batpak::encoding::from_bytes::<SourceSnapshotRelationV1>(
            &raw.event.payload,
        )?);
    }
    let first_id: texo::knowledge::SourceSnapshotId =
        serde_json::from_value(first["snapshot_id"].clone())?;
    let second_id: texo::knowledge::SourceSnapshotId =
        serde_json::from_value(second["snapshot_id"].clone())?;
    let third_id: texo::knowledge::SourceSnapshotId =
        serde_json::from_value(third["snapshot_id"].clone())?;
    assert!(relations.iter().any(|relation| {
        relation.left_snapshot_id == first_id
            && relation.right_snapshot_id == second_id
            && relation.relation == texo::knowledge::TemporalRelation::Before
    }));
    assert!(relations.iter().any(|relation| {
        relation.left_snapshot_id == second_id
            && relation.right_snapshot_id == third_id
            && relation.relation == texo::knowledge::TemporalRelation::Concurrent
    }));
    let frontier = host
        .store()
        .query_entries_after(&region, None, 256)
        .last()
        .map(batpak::store::IndexEntry::global_sequence)
        .ok_or("workspace frontier")?;
    let replayed =
        texo::claims::temporal::assemble_through(&host.store(), host.workspace_id(), frontier)?;
    assert_eq!(
        replayed.compare(&first_id, &second_id),
        texo::knowledge::TemporalRelation::Before
    );
    assert_eq!(
        replayed.compare(&second_id, &first_id),
        texo::knowledge::TemporalRelation::After
    );
    assert_eq!(
        replayed.compare(&second_id, &third_id),
        texo::knowledge::TemporalRelation::Concurrent
    );
    Ok(())
}

#[test]
fn worktree_overlays_preserve_partial_order_without_false_ancestry() -> TestResult {
    let (root, mut host) = initialized_repository()?;
    let first = host.invoke_json(
        "texo.knowledge.index",
        &json!({"observed_at_ms": 1_700_000_000_002_u64}),
    )?;
    std::fs::write(
        root.path().join("src/lib.rs"),
        "pub fn deploy() { helper(); }\nfn helper() {}\nfn overlay_one() {}\n",
    )?;
    let second = host.invoke_json(
        "texo.knowledge.index",
        &json!({"observed_at_ms": 1_700_000_000_003_u64}),
    )?;
    std::fs::write(
        root.path().join("src/lib.rs"),
        "pub fn deploy() { helper(); }\nfn helper() {}\nfn overlay_two() {}\n",
    )?;
    let third = host.invoke_json(
        "texo.knowledge.index",
        &json!({"observed_at_ms": 1_700_000_000_004_u64}),
    )?;

    let first_id: texo::knowledge::SourceSnapshotId =
        serde_json::from_value(first["snapshot_id"].clone())?;
    let second_id: texo::knowledge::SourceSnapshotId =
        serde_json::from_value(second["snapshot_id"].clone())?;
    let third_id: texo::knowledge::SourceSnapshotId =
        serde_json::from_value(third["snapshot_id"].clone())?;
    let region = Region::scope(scope_for_workspace("demo"));
    let frontier = host
        .store()
        .query_entries_after(&region, None, 256)
        .last()
        .map(batpak::store::IndexEntry::global_sequence)
        .ok_or("workspace frontier")?;
    let replayed =
        texo::claims::temporal::assemble_through(&host.store(), host.workspace_id(), frontier)?;

    assert_eq!(
        replayed.compare(&first_id, &second_id),
        texo::knowledge::TemporalRelation::Before,
        "a clean base is causally before its frozen overlay"
    );
    assert_eq!(
        replayed.compare(&second_id, &third_id),
        texo::knowledge::TemporalRelation::Concurrent,
        "distinct overlays on one base are alternatives, not a time sequence"
    );
    Ok(())
}
