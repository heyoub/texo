//! Multi-journal topology and authority-boundary integration witnesses.

use std::collections::BTreeMap;

use serde_json::json;
use texo::config::{TexoRootConfig, WorkspaceEntry};
use texo::host::TexoHost;
use texo::topology::{JournalEntry, JournalRole, ReplicaMode};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn write_topology(root: &std::path::Path) -> TestResult {
    let mut journals = BTreeMap::new();
    journals.insert(
        "canonical".to_string(),
        JournalEntry::canonical(".texo/store"),
    );
    journals.insert(
        "codex".to_string(),
        JournalEntry::replica(
            ".texo/replicas/codex",
            "canonical",
            ReplicaMode::ImportedReadModel,
        ),
    );
    let config = TexoRootConfig {
        default_workspace: "demo".to_string(),
        workspaces: BTreeMap::from([(
            "demo".to_string(),
            WorkspaceEntry {
                primary_journal: "canonical".to_string(),
                journals,
                docs_glob: "docs/**/*.md".to_string(),
                extractor_cmd: None,
                semantics: None,
            },
        )]),
        gateway: None,
    };
    config.save(&root.join(".texo/config.toml"))?;
    Ok(())
}

#[test]
fn canonical_and_replica_open_together_with_separate_authority() -> TestResult {
    let root = tempfile::tempdir()?;
    write_topology(root.path())?;
    let mut canonical = TexoHost::open_journal(root.path(), "demo", "canonical", 1)?;
    let mut replica = TexoHost::open_journal(root.path(), "demo", "codex", 1)?;

    assert_eq!(canonical.journal_role(), JournalRole::Canonical);
    assert_eq!(replica.journal_role(), JournalRole::Replica);
    assert_eq!(replica.journal_id().as_str(), "codex");
    assert_eq!(
        canonical.fingerprints().module_digest,
        replica.fingerprints().module_digest,
        "role policy is one declared module contract, not a forked interface"
    );
    let physical_frontier = replica.store().frontier().visible_hlc.global_sequence;

    let _init = canonical.invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))?;
    let reloaded = TexoRootConfig::load(&root.path().join(".texo/config.toml"))?;
    assert!(reloaded.workspaces["demo"].journals.contains_key("codex"));
    let before = replica.store().by_scope("workspace:demo").len();
    let denied = replica
        .invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))
        .expect_err("replica must refuse authority-bearing operations");
    assert_eq!(denied.code(), "op.denied");
    assert!(denied.to_string().contains("journal.read_only_replica"));
    assert_eq!(replica.store().by_scope("workspace:demo").len(), before);

    let claims = replica.invoke_json("texo.claims.list", &json!({"subject": null}))?;
    assert_eq!(claims["claims"], json!([]));
    assert_eq!(
        replica.store().frontier().visible_hlc.global_sequence,
        physical_frontier,
        "opening and reading a replica must append no lifecycle, status, receipt, or domain event"
    );
    drop(replica);
    let reopened = TexoHost::open_journal(root.path(), "demo", "codex", 2)?;
    assert_eq!(
        reopened.store().frontier().visible_hlc.global_sequence,
        physical_frontier,
        "closing and reopening a replica must preserve its complete physical frontier"
    );
    Ok(())
}

#[test]
fn snapshot_tokens_refuse_a_different_physical_journal() -> TestResult {
    let root = tempfile::tempdir()?;
    write_topology(root.path())?;
    let mut canonical = TexoHost::open_journal(root.path(), "demo", "canonical", 1)?;
    let _init = canonical.invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))?;
    let context = canonical.invoke_json(
        "texo.context.agent",
        &json!({"subject": null, "include_stale": false, "allow_unsettled": true}),
    )?;
    let token = context["snapshot"]["token"]
        .as_str()
        .ok_or("snapshot token")?
        .to_string();
    drop(canonical);

    let mut replica = TexoHost::open_journal(root.path(), "demo", "codex", 2)?;
    let error = replica
        .invoke_json(
            "texo.context.agent",
            &json!({
                "subject": null,
                "include_stale": false,
                "snapshot": token
            }),
        )
        .expect_err("journal-affine token must not cross into a replica");
    assert_eq!(error.code(), "snapshot.invalid");
    Ok(())
}
