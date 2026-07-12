//! Cached proposal-only claim↔code reconciliation contracts.

use std::path::Path;
use std::process::{Command, Output};

use batpak::coordinate::Region;
use batpak::event::EventPayload;
use batpak::id::EntityIdType;
use serde_json::{json, Value};
use tempfile::TempDir;
use texo::events::coordinate::scope_for_workspace;
use texo::events::ids::{blake3_hash_hex, ClaimId};
use texo::events::payloads::{
    ClaimEvidenceLinkedV1, EvidenceOccurrenceRecordedV1, EvidenceReconciliationAcceptedV1,
};
use texo::knowledge::{CodeIndexId, EvidenceLinkMethod};
use texo::reconcile::{plan_candidates, ReconcileClaim, ReconcileLimits};
use texo::semantics::{ClaimRelation, RelationVerdict};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn git(root: &Path, args: &[&str]) -> TestResult {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

fn texo(root: &Path, args: &[&str]) -> TestResult<Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_texo"))
        .arg("--root")
        .arg(root)
        .arg("--workspace")
        .arg("demo")
        .args(args)
        .output()?)
}

fn repository() -> TestResult<TempDir> {
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
        root.path().join("docs/retries.md"),
        "Decision: retries are limited to four attempts.\n",
    )?;
    std::fs::write(
        root.path().join("src/config.rs"),
        "pub fn max_retries() -> usize { 4 }\n",
    )?;
    git(root.path(), &["add", "docs/retries.md", "src/config.rs"])?;
    git(root.path(), &["commit", "-qm", "retry policy"])?;
    Ok(root)
}

struct PreparedWorkspace {
    root: TempDir,
    claim_id: ClaimId,
    cache: std::path::PathBuf,
}

fn prepared_workspace(relation: ClaimRelation) -> TestResult<PreparedWorkspace> {
    let root = repository()?;
    assert!(texo(root.path(), &["init"])?.status.success());
    assert!(texo(root.path(), &["ingest", "docs/retries.md", "--json"])?
        .status
        .success());
    let indexed = texo(root.path(), &["index", "--json"])?;
    assert!(indexed.status.success());
    let indexed: Value = serde_json::from_slice(&indexed.stdout)?;
    let index_id: CodeIndexId = serde_json::from_value(indexed["code"]["index_id"].clone())?;
    let artifact_digest = indexed["code"]["artifact_digest_hex"]
        .as_str()
        .ok_or("artifact digest")?;
    let artifact =
        texo::code_index::load(root.path(), &index_id, artifact_digest)?.ok_or("code artifact")?;
    let claims = texo(root.path(), &["claims", "--json"])?;
    let claims: Value = serde_json::from_slice(&claims.stdout)?;
    let claim = claims
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["status"] == "current"))
        .ok_or("current claim")?;
    let claim_id = ClaimId::try_from(claim["claim_id"].as_str().ok_or("claim id")?)?;
    let plan = plan_candidates(
        &[ReconcileClaim {
            claim_id: claim_id.clone(),
            text: claim["text"].as_str().ok_or("claim text")?.to_string(),
            subject_hint: claim["subject_hint"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            predicate_hint: String::new(),
            object_hint: String::new(),
        }],
        &artifact,
        ReconcileLimits::default(),
    );
    assert!(
        !plan.candidates.is_empty(),
        "fixture must produce code candidates"
    );
    let cache = root.path().join("reconcile-cache");
    std::fs::create_dir_all(&cache)?;
    let fingerprint = "openrouter:test/reconcile|relation-v2";
    for candidate in &plan.candidates {
        let key = blake3_hash_hex(&format!(
            "{fingerprint}\u{1f}{}\u{1f}{}",
            candidate.claim_text, candidate.code_prompt
        ));
        std::fs::write(
            cache.join(format!("{key}.json")),
            serde_json::to_vec(&RelationVerdict {
                relation,
                score: 0.99,
            })?,
        )?;
    }
    Ok(PreparedWorkspace {
        root,
        claim_id,
        cache,
    })
}

fn run_reconcile(root: &Path, cache: &Path) -> TestResult<Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_texo"))
        .arg("--root")
        .arg(root)
        .arg("--workspace")
        .arg("demo")
        .args(["reconcile", "--json"])
        .env("TEXO_LLM_API_KEY", "test-key-not-secret")
        .env("TEXO_LLM_RELATE_MODEL", "test/reconcile")
        .env("TEXO_LLM_BASE_URL", "http://127.0.0.1:9/v1")
        .env("TEXO_RECONCILE_CACHE", cache)
        .output()?)
}

fn assert_backup_restore_preserves_explanation(root: &Path, claim_id: &ClaimId) -> TestResult {
    let outside = TempDir::new()?;
    let backup = outside.path().join("backup");
    let restored = outside.path().join("restored");
    let backup_text = backup.to_str().ok_or("backup path")?;
    let created = texo(root, &["backup", "create", backup_text, "--json"])?;
    assert!(created.status.success());
    let restored_output = texo(&restored, &["backup", "restore", backup_text, "--json"])?;
    assert!(
        restored_output.status.success(),
        "{}",
        String::from_utf8_lossy(&restored_output.stderr)
    );
    assert!(!restored.join(".texo/cache").exists());
    let mut host = texo::host::TexoHost::open(&restored, "demo", 1_700_000_000_102)?;
    let explained = host.invoke_json(
        "texo.claim.explain",
        &json!({"claim_id": claim_id, "snapshot": null}),
    )?;
    assert!(explained["evidence"].as_array().is_some_and(|rows| {
        rows.iter().any(|row| {
            row["method"] == "semantic_policy"
                && row["occurrence"]["path"] == "src/config.rs"
                && row["reconciliation"]["policy_version"] == "evidence-reconcile-v1"
        })
    }));
    Ok(())
}

#[test]
fn cached_proposal_is_policy_accepted_once_and_survives_artifact_deletion() -> TestResult {
    let PreparedWorkspace {
        root,
        claim_id,
        cache,
    } = prepared_workspace(ClaimRelation::Duplicate)?;

    let first = run_reconcile(root.path(), &cache)?;
    assert!(first.status.success() || first.status.code() == Some(2));
    let first: Value = serde_json::from_slice(&first.stdout)?;
    assert!(first["accepted"]
        .as_array()
        .is_some_and(|rows| !rows.is_empty()));

    let mut host = texo::host::TexoHost::open(root.path(), "demo", 1_700_000_000_100)?;
    let explained = host.invoke_json(
        "texo.claim.explain",
        &json!({"claim_id": claim_id, "snapshot": null}),
    )?;
    assert!(
        explained["evidence"].as_array().is_some_and(|rows| {
            rows.iter().any(|row| {
                row["method"] == "semantic_policy"
                    && row["occurrence"]["path"] == "src/config.rs"
                    && row["occurrence"]["excerpt"]
                        .as_str()
                        .is_some_and(|excerpt| excerpt.contains("{ 4 }"))
                    && row["reconciliation"]["judge_fingerprint"]
                        == "openrouter:test/reconcile|relation-v2"
                    && row["reconciliation"]["policy_version"] == "evidence-reconcile-v1"
            })
        }),
        "{explained}"
    );

    let region = Region::scope(scope_for_workspace("demo"));
    let entries = host.store().query_entries_after(&region, None, 256);
    let accepted = entries
        .iter()
        .find(|entry| {
            entry.event_kind() == <EvidenceReconciliationAcceptedV1 as EventPayload>::KIND
        })
        .ok_or("acceptance event")?;
    let occurrence = entries
        .iter()
        .find(|entry| {
            entry.event_kind() == <EvidenceOccurrenceRecordedV1 as EventPayload>::KIND
                && accepted.causation_id() == Some(entry.event_id().as_u128())
        })
        .ok_or("causal occurrence event")?;
    let link = entries
        .iter()
        .rev()
        .find(|entry| entry.event_kind() == <ClaimEvidenceLinkedV1 as EventPayload>::KIND)
        .ok_or("link event")?;
    assert_eq!(
        accepted.causation_id(),
        Some(occurrence.event_id().as_u128())
    );
    assert_eq!(link.causation_id(), Some(accepted.event_id().as_u128()));
    let raw = host.store().read_raw(accepted.event_id())?;
    let provenance: EvidenceReconciliationAcceptedV1 =
        batpak::encoding::from_bytes(&raw.event.payload)?;
    assert_eq!(provenance.policy_version, "evidence-reconcile-v1");
    let raw = host.store().read_raw(link.event_id())?;
    let link_payload: ClaimEvidenceLinkedV1 = batpak::encoding::from_bytes(&raw.event.payload)?;
    assert_eq!(link_payload.method, EvidenceLinkMethod::SemanticPolicy);
    let count_before = entries.len();
    drop(host);

    std::fs::remove_dir_all(&cache)?;
    let second = run_reconcile(root.path(), &cache)?;
    assert!(second.status.success() || second.status.code() == Some(2));
    let second: Value = serde_json::from_slice(&second.stdout)?;
    assert!(second["already_linked"]
        .as_u64()
        .is_some_and(|count| count > 0));

    std::fs::remove_dir_all(root.path().join(".texo/cache/code-index"))?;

    let mut reopened = texo::host::TexoHost::open(root.path(), "demo", 1_700_000_000_101)?;
    assert_eq!(
        reopened
            .store()
            .query_entries_after(&region, None, 256)
            .len(),
        count_before,
        "resume after deleting caches must append no duplicate facts"
    );
    let explained = reopened.invoke_json(
        "texo.claim.explain",
        &json!({"claim_id": claim_id, "snapshot": null}),
    )?;
    assert!(explained["evidence"]
        .as_array()
        .is_some_and(|rows| { rows.iter().any(|row| row["method"] == "semantic_policy") }));
    drop(reopened);
    assert_backup_restore_preserves_explanation(root.path(), &claim_id)?;
    Ok(())
}

#[test]
fn contradictory_code_evidence_changes_answer_not_claim_lifecycle() -> TestResult {
    let PreparedWorkspace {
        root,
        claim_id,
        cache,
    } = prepared_workspace(ClaimRelation::Conflict)?;
    let output = run_reconcile(root.path(), &cache)?;
    assert!(output.status.success() || output.status.code() == Some(2));

    let mut host = texo::host::TexoHost::open(root.path(), "demo", 1_700_000_000_200)?;
    let explained = host.invoke_json(
        "texo.claim.explain",
        &json!({"claim_id": claim_id, "snapshot": null}),
    )?;
    assert_eq!(explained["answer_state"], "contradicted");
    assert_eq!(explained["card"]["phase"], 1);
    assert!(explained["evidence"].as_array().is_some_and(|rows| {
        rows.iter()
            .any(|row| row["method"] == "semantic_policy" && row["stance"] == "contradicts")
    }));
    Ok(())
}
