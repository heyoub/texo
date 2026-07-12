//! Journaled Git snapshot and claim-evidence integration.

use std::path::Path;
use std::process::Command;

use batpak::coordinate::Region;
use batpak::event::{EventKind, EventPayload};
use batpak::id::EntityIdType;
use serde_json::json;
use tempfile::TempDir;
use texo::events::coordinate::scope_for_workspace;
use texo::events::payloads::{
    ClaimEvidenceLinkedV1, EvidenceOccurrenceRecordedV1, SourceSnapshotRecordedV1,
};
use texo::host::TexoHost;

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

fn initialized_repository() -> TestResult<(TempDir, TexoHost)> {
    let root = TempDir::new()?;
    git(root.path(), &["init", "-q"])?;
    git(root.path(), &["config", "user.name", "Texo Test"])?;
    git(
        root.path(),
        &["config", "user.email", "texo@example.invalid"],
    )?;
    std::fs::create_dir_all(root.path().join("docs"))?;
    std::fs::write(
        root.path().join("docs/decision.md"),
        "Decision: deploys happen on Friday.\n",
    )?;
    git(root.path(), &["add", "docs/decision.md"])?;
    git(root.path(), &["commit", "-qm", "initial"])?;

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
