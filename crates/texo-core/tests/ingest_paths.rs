//! PROVES: the public ingest entry points (`ingest_sources`,
//! `plan_ingest_sources`) and their dry-run / external-extractor branches behave
//! correctly against a REAL BatPak store and REAL filesystem sources — real stores only.
//! Also exercises the cross-session staleness, agent-context and explain paths
//! that only fire once claims are genuinely superseded in a committed journal.

mod support;

use std::collections::HashSet;
use std::path::Path;

use assert_matches::assert_matches;
use support::temp_workspace;
use texo_core::{
    build_agent_context, check_staleness, explain_claim, ingest_sources, plan_ingest_sources,
    ClaimId, ClaimStatus, IngestMode, Journal, Open, SemanticsConfig, WorkspaceConfig, WorkspaceId,
    FIXTURE_OBSERVED_AT_MS,
};

/// Open a journal whose config uses the heuristic extractor against a store and
/// docs tree both rooted under `root`.
fn open_heuristic_journal(root: &Path) -> Journal<Open> {
    let config = WorkspaceConfig {
        workspace_id: "demo".to_string(),
        store_path: ".texo/store".to_string(),
        docs_glob: "docs/**/*.md".to_string(),
        extractor_cmd: None,
        semantics: None,
    };
    Journal::<Open>::open(config, root).expect("open journal")
}

fn write_doc(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir docs");
    std::fs::write(path, body).expect("write doc");
}

#[test]
fn commit_then_dry_run_is_idempotent_and_writes_no_new_events() {
    let dir = temp_workspace();
    let root = dir.path();
    write_doc(root, "docs/policy.md", "Deploys happen on Friday.\n");

    let journal = open_heuristic_journal(root);
    let workspace = journal.config().workspace().expect("workspace");

    // First commit records the source and at least one claim.
    let committed = ingest_sources(
        journal.handle(),
        journal.config(),
        &workspace,
        &root.join("docs"),
        IngestMode::Commit,
        FIXTURE_OBSERVED_AT_MS,
        root,
    )
    .expect("commit ingest");
    assert_eq!(committed.sources_observed, 1);
    assert!(committed.claims_recorded >= 1);
    assert_eq!(
        committed.receipts.len(),
        committed.sources_observed + committed.claims_recorded,
        "every observed source and recorded claim emits exactly one receipt"
    );

    // A DryRun over the SAME unchanged source observes nothing new and writes no
    // receipts (the dedup-by-body-hash + DryRun early-return path).
    let planned = ingest_sources(
        journal.handle(),
        journal.config(),
        &workspace,
        &root.join("docs"),
        IngestMode::DryRun,
        FIXTURE_OBSERVED_AT_MS,
        root,
    )
    .expect("dry-run ingest");
    assert_eq!(
        planned.sources_observed, 0,
        "already-ingested source must be skipped"
    );
    assert!(
        planned.receipts.is_empty(),
        "dry-run must never emit receipts"
    );

    journal.close().expect("close");
}

#[test]
fn dry_run_on_fresh_source_plans_without_committing() {
    let dir = temp_workspace();
    let root = dir.path();
    write_doc(root, "docs/policy.md", "Deploys happen on Friday.\n");

    let journal = open_heuristic_journal(root);
    let workspace = journal.config().workspace().expect("workspace");

    // DryRun on a never-ingested source reports the would-be counts but writes
    // nothing — proving the IngestMode::DryRun early return in ingest_sources.
    let planned = ingest_sources(
        journal.handle(),
        journal.config(),
        &workspace,
        &root.join("docs"),
        IngestMode::DryRun,
        FIXTURE_OBSERVED_AT_MS,
        root,
    )
    .expect("dry-run ingest");
    assert_eq!(planned.sources_observed, 1);
    assert!(planned.claims_recorded >= 1);
    assert!(planned.receipts.is_empty());

    // Nothing was committed: replay sees zero claims.
    let replayed = journal.replay(&workspace).expect("replay");
    assert!(
        replayed.state.claims.is_empty(),
        "dry-run must not persist any claim"
    );

    journal.close().expect("close");
}

#[test]
fn plan_ingest_sources_reports_counts_without_writing() {
    let dir = temp_workspace();
    let root = dir.path();
    write_doc(root, "docs/policy.md", "Deploys happen on Friday.\n");

    let journal = open_heuristic_journal(root);
    let workspace = journal.config().workspace().expect("workspace");
    let empty: HashSet<String> = HashSet::new();

    let plan = plan_ingest_sources(
        journal.handle(),
        &root.join("docs"),
        journal.config(),
        &workspace,
        FIXTURE_OBSERVED_AT_MS,
        &empty,
        root,
    )
    .expect("plan ingest");
    assert_eq!(plan.sources_observed, 1);
    assert!(plan.claims_recorded >= 1);

    // plan_ingest_sources must not write: the store still replays empty.
    let replayed = journal.replay(&workspace).expect("replay");
    assert!(replayed.state.claims.is_empty());

    journal.close().expect("close");
}

#[test]
fn external_extractor_cmd_path_records_its_claims() {
    // Drive the config.extractor_cmd branch (ingest.rs extract_for_doc ->
    // extract_via_cmd) with a REAL shell command emitting one JSON claim.
    let dir = temp_workspace();
    let root = dir.path();
    write_doc(root, "docs/policy.md", "Anything at all here.\n");

    let config = WorkspaceConfig {
        workspace_id: "demo".to_string(),
        store_path: ".texo/store".to_string(),
        docs_glob: "docs/**/*.md".to_string(),
        extractor_cmd: Some(
            r#"printf '{"line_start": 1, "text": "Deploys moved to Tuesday.", "subject_hint": "deploy-process"}\n'; :"#
                .to_string(),
        ),
        semantics: None,
    };
    let journal = Journal::<Open>::open(config, root).expect("open journal");
    let workspace = journal.config().workspace().expect("workspace");

    let committed = ingest_sources(
        journal.handle(),
        journal.config(),
        &workspace,
        &root.join("docs"),
        IngestMode::Commit,
        FIXTURE_OBSERVED_AT_MS,
        root,
    )
    .expect("commit ingest via cmd extractor");
    assert_eq!(committed.sources_observed, 1);
    assert_eq!(
        committed.claims_recorded, 1,
        "the cmd extractor emitted exactly one claim"
    );

    // The recorded claim must carry the extractor-supplied text via replay.
    let replayed = journal.replay(&workspace).expect("replay");
    let texts: Vec<&str> = replayed
        .state
        .claims
        .values()
        .map(|c| c.text.as_str())
        .collect();
    assert_eq!(texts, vec!["Deploys moved to Tuesday."]);
    let kinds: HashSet<&str> = replayed
        .state
        .claims
        .values()
        .map(|c| c.extractor_kind.as_str())
        .collect();
    assert!(
        kinds.iter().any(|k| k.starts_with("cmd:")),
        "claim must be tagged with the cmd extractor kind, got {kinds:?}"
    );

    journal.close().expect("close");
}

#[test]
fn failing_external_extractor_surfaces_error_through_ingest() {
    // A cmd extractor that exits non-zero must propagate as a JournalError
    // (Extract) out of ingest_sources, not be silently dropped. This drives the
    // error-propagation `?` in ingest::plan_sources' extract_for_doc call.
    let dir = temp_workspace();
    let root = dir.path();
    write_doc(root, "docs/policy.md", "Anything here.\n");

    let config = WorkspaceConfig {
        workspace_id: "demo".to_string(),
        store_path: ".texo/store".to_string(),
        docs_glob: "docs/**/*.md".to_string(),
        // No JSON emitted and a non-zero exit -> the extractor reports failure.
        extractor_cmd: Some(r"exit 7; :".to_string()),
        semantics: None,
    };
    let journal = Journal::<Open>::open(config, root).expect("open journal");
    let workspace = journal.config().workspace().expect("workspace");

    let err = ingest_sources(
        journal.handle(),
        journal.config(),
        &workspace,
        &root.join("docs"),
        IngestMode::Commit,
        FIXTURE_OBSERVED_AT_MS,
        root,
    )
    .expect_err("failing extractor must surface an error");
    assert_matches!(err, texo_core::JournalError::Extract(_));

    journal.close().expect("close");
}

#[test]
fn semantics_enabled_suppresses_heuristic_supersession() {
    // The keyword heuristic supersedes "Deploys happen on Friday" with the later
    // "Deploys moved to Tuesday". With `[semantics].enabled` the dedicated relate
    // pass is authoritative, so that heuristic supersession must be suppressed —
    // otherwise the two passes fight and bury claims the wrong way.
    fn superseded_after_ingest(root: &Path, semantics_enabled: bool) -> usize {
        write_doc(root, "docs/a.md", "Deploys happen on Friday.\n");
        write_doc(root, "docs/b.md", "Deploys moved to Tuesday.\n");
        let semantics = semantics_enabled.then(|| SemanticsConfig {
            enabled: true,
            ..SemanticsConfig::default()
        });
        let config = WorkspaceConfig {
            workspace_id: "demo".to_string(),
            store_path: ".texo/store".to_string(),
            docs_glob: "docs/**/*.md".to_string(),
            extractor_cmd: None,
            semantics,
        };
        let journal = Journal::<Open>::open(config, root).expect("open journal");
        let workspace = journal.config().workspace().expect("workspace");
        ingest_sources(
            journal.handle(),
            journal.config(),
            &workspace,
            &root.join("docs"),
            IngestMode::Commit,
            FIXTURE_OBSERVED_AT_MS,
            root,
        )
        .expect("ingest");
        let replayed = journal.replay(&workspace).expect("replay");
        let superseded = replayed
            .state
            .claims
            .values()
            .filter(|c| c.status != ClaimStatus::Current)
            .count();
        journal.close().expect("close");
        superseded
    }

    let off_dir = temp_workspace();
    let on_dir = temp_workspace();
    let off = superseded_after_ingest(off_dir.path(), false);
    let on = superseded_after_ingest(on_dir.path(), true);

    assert!(
        off >= 1,
        "non-vacuous: the heuristic must supersede with semantics OFF (got {off})"
    );
    assert_eq!(
        on, 0,
        "semantics ENABLED must suppress heuristic supersession (got {on})"
    );
}

#[test]
fn cmd_extractor_resolves_doc_path_from_its_cwd() {
    // Regression: the external extractor runs with its cwd at the docs scan dir,
    // so a relative `doc.path` ("policy.md" under a docs/ subdir) resolves to a
    // real file. The extractor below READS "$1" (unlike the printf cases, which
    // ignore it) and only emits a claim if the read succeeds — so if the path did
    // not resolve from the extractor's cwd, `cat` would fail, the command would
    // exit non-zero, and ingest would surface an Extract error instead.
    let dir = temp_workspace();
    let root = dir.path();
    write_doc(root, "docs/policy.md", "Deploys moved to Tuesday.\n");

    let config = WorkspaceConfig {
        workspace_id: "demo".to_string(),
        store_path: ".texo/store".to_string(),
        docs_glob: "docs/**/*.md".to_string(),
        extractor_cmd: Some(
            r#"cat "$1" >/dev/null && printf '{"line_start": 1, "text": "Deploys moved to Tuesday."}\n'"#
                .to_string(),
        ),
        semantics: None,
    };
    let journal = Journal::<Open>::open(config, root).expect("open journal");
    let workspace = journal.config().workspace().expect("workspace");

    let committed = ingest_sources(
        journal.handle(),
        journal.config(),
        &workspace,
        &root.join("docs"),
        IngestMode::Commit,
        FIXTURE_OBSERVED_AT_MS,
        root,
    )
    .expect("the extractor must be able to read the doc from its cwd");
    assert_eq!(committed.claims_recorded, 1);

    let replayed = journal.replay(&workspace).expect("replay");
    let texts: Vec<String> = replayed
        .state
        .claims
        .values()
        .map(|c| c.text.clone())
        .collect();
    assert_eq!(texts, vec!["Deploys moved to Tuesday."]);
    journal.close().expect("close");
}

#[test]
fn supersession_marks_old_claim_stale_and_agent_context_reflects_it() {
    // Two ingests on the same subject across sessions: the second supersedes the
    // first. This exercises check_staleness's superseder-present branches, the
    // agent-context Superseded branch, and explain_claim's superseded_by output.
    let dir = temp_workspace();
    let root = dir.path();

    write_doc(root, "docs/v1.md", "Deploys happen on Friday.\n");
    {
        let journal = open_heuristic_journal(root);
        let workspace = journal.config().workspace().expect("workspace");
        ingest_sources(
            journal.handle(),
            journal.config(),
            &workspace,
            &root.join("docs"),
            IngestMode::Commit,
            FIXTURE_OBSERVED_AT_MS,
            root,
        )
        .expect("first ingest");
        journal.close().expect("close");
    }

    // Second source on the same subject with a replacement keyword.
    write_doc(root, "docs/v2.md", "Deploys moved to Tuesday.\n");
    let journal = open_heuristic_journal(root);
    let workspace = journal.config().workspace().expect("workspace");
    ingest_sources(
        journal.handle(),
        journal.config(),
        &workspace,
        &root.join("docs"),
        IngestMode::Commit,
        FIXTURE_OBSERVED_AT_MS,
        root,
    )
    .expect("second ingest");

    let replayed = journal.replay(&workspace).expect("replay");

    // Exactly one claim is now Superseded; locate it and its superseder.
    let stale: Vec<&texo_core::ClaimView> = replayed
        .state
        .claims
        .values()
        .filter(|c| c.status == ClaimStatus::Superseded)
        .collect();
    assert_eq!(stale.len(), 1, "one claim must be superseded");
    let stale_claim = stale[0];
    assert!(
        stale_claim.superseded_by.is_some(),
        "superseded claim must name its superseder"
    );

    // check_staleness over the v1 file must flag the stale claim with a receipt.
    let report = check_staleness(&replayed.state, &workspace, &root.join("docs/v1.md"), root)
        .expect("staleness");
    assert_eq!(report.diagnostics.len(), 1, "v1 claim flagged stale");
    let diag = &report.diagnostics[0];
    assert_eq!(diag.claim_id, stale_claim.claim_id);
    assert!(diag.superseded_by.is_some());
    assert!(
        diag.message.contains("superseded by"),
        "message must explain supersession: {}",
        diag.message
    );
    assert!(
        diag.source.is_some(),
        "superseder source provenance must be present"
    );
    assert!(
        diag.receipt.is_some(),
        "supersession receipt must be present"
    );

    // Agent context: the stale claim appears under stale_claims, the winner under
    // current claims.
    let context = build_agent_context(&replayed.state, &workspace, None);
    assert_eq!(context.stale_claims.len(), 1);
    assert_eq!(context.stale_claims[0].claim_id, stale_claim.claim_id);
    assert!(
        context
            .claims
            .iter()
            .all(|c| c.status == ClaimStatus::Current),
        "agent current claims must all be Current"
    );

    // Subject filter narrows to the deploy-process subject and still returns both
    // the current winner and (via stale_claims) the loser.
    let filtered = build_agent_context(&replayed.state, &workspace, Some("deploy-process"));
    assert!(
        filtered
            .claims
            .iter()
            .all(|c| c.subject_hint == "deploy-process"),
        "subject filter must restrict current claims"
    );
    // A non-matching filter yields no current claims (subject_filter mismatch arm).
    let none = build_agent_context(&replayed.state, &workspace, Some("nonexistent-subject"));
    assert!(none.claims.is_empty(), "filter mismatch yields no claims");

    // explain_claim on the stale claim surfaces its superseded_by edge.
    let explanation =
        explain_claim(&replayed.state, &stale_claim.claim_id).expect("explanation for known claim");
    assert_eq!(explanation.superseded_by.len(), 1);
    assert_eq!(explanation.status, ClaimStatus::Superseded);

    // explain_claim on an unknown id returns None (claim() miss path).
    let ghost = ClaimId::try_from("claim_ffffffffffff").expect("id");
    assert!(explain_claim(&replayed.state, &ghost).is_none());

    journal.close().expect("close");
}

#[test]
fn check_staleness_strips_root_prefix_for_checked_path() {
    // When input is under root, checked_path is the relative remainder.
    let dir = temp_workspace();
    let root = dir.path();
    write_doc(root, "docs/policy.md", "Deploys happen on Friday.\n");

    let journal = open_heuristic_journal(root);
    let workspace = journal.config().workspace().expect("workspace");
    let replayed = journal.replay(&workspace).expect("replay");

    let report =
        check_staleness(&replayed.state, &workspace, &root.join("docs"), root).expect("staleness");
    assert_eq!(report.checked_path, "docs");

    journal.close().expect("close");
}

#[test]
fn ingest_of_file_at_root_uses_dot_parent() {
    // Passing a single markdown FILE (not a dir) whose parent is the temp root
    // exercises the `input.parent()` branch in plan_sources.
    let dir = temp_workspace();
    let root = dir.path();
    let file = root.join("solo.md");
    std::fs::write(&file, "Deploys happen on Friday.\n").expect("write");

    let config = WorkspaceConfig {
        workspace_id: "demo".to_string(),
        store_path: ".texo/store".to_string(),
        docs_glob: "**/*.md".to_string(),
        extractor_cmd: None,
        semantics: None,
    };
    let journal = Journal::<Open>::open(config, root).expect("open journal");
    let workspace = WorkspaceId::new("demo").expect("workspace");

    let committed = ingest_sources(
        journal.handle(),
        journal.config(),
        &workspace,
        &file,
        IngestMode::Commit,
        FIXTURE_OBSERVED_AT_MS,
        root,
    )
    .expect("ingest single file");
    assert_eq!(committed.sources_observed, 1);

    journal.close().expect("close");
}
