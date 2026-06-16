//! Full-pipeline end-to-end smoke test driving the `texo` binary against a
//! tempdir: init -> ingest -> claims -> agent-context (default + --json) ->
//! check-staleness -> compile -> conflicts (read + --commit) -> verify ->
//! supersede.
//!
//! Every step asserts a real outcome: exit codes, specific stdout/JSON fields,
//! and files written. The conflicts/verify/supersede subcommands have no other
//! coverage, so this test is what lifts them (and CLI lib.rs + the journal
//! append/state paths they exercise) off 0%.

use assert_cmd::cargo::cargo_bin;
use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;
use texo_core::fixture::FIXTURE_OBSERVED_AT_MS;

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// One pinned-clock `texo` invocation rooted at `dir`.
fn texo(dir: &Path) -> Command {
    let mut cmd = Command::new(cargo_bin("texo"));
    cmd.env("TEXO_OBSERVED_AT_MS", FIXTURE_OBSERVED_AT_MS.to_string())
        .current_dir(dir);
    cmd
}

/// Copy the repo's `sample_sources/*.md` into `dir/sample_sources`.
fn seed_sources(dir: &Path) {
    let sample = repo_root().join("sample_sources");
    let dest = dir.join("sample_sources");
    std::fs::create_dir_all(&dest).expect("mkdir sample_sources");
    for entry in std::fs::read_dir(&sample).expect("read sample_sources") {
        let entry = entry.expect("entry");
        std::fs::copy(entry.path(), dest.join(entry.file_name())).expect("copy sample");
    }
}

/// Parse stdout of a successful command as JSON, failing loudly otherwise.
fn json_stdout(cmd: &mut Command) -> Value {
    let out = cmd.output().expect("run command");
    assert!(
        out.status.success(),
        "command failed: status={:?} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "expected JSON on stdout, got error {e}; stdout={}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// init -> ingest: assert the store is created and claims are recorded.
fn phase_init_and_ingest(root: &Path) {
    texo(root)
        .args(["init", "--workspace", "demo"])
        .assert()
        .success();
    assert!(
        root.join(".texo").is_dir(),
        "init must create the .texo config/store directory"
    );

    let ingest = json_stdout(texo(root).args(["ingest", "sample_sources", "--json"]));
    assert!(
        ingest["claims_recorded"].as_u64().unwrap_or(0) > 0,
        "ingesting sample_sources must record at least one claim, got {ingest}"
    );
}

/// claims --json: assert all surfaced claims are current with sequenced
/// receipts, and return two distinct claim ids for the later supersede.
fn phase_claims(root: &Path) -> (String, String) {
    let claims = json_stdout(texo(root).args(["claims", "--json"]));
    let claims_arr = claims.as_array().expect("claims --json must be an array");
    assert!(
        !claims_arr.is_empty(),
        "claims --json must list at least one current claim"
    );
    for claim in claims_arr {
        assert_eq!(
            claim["status"].as_str(),
            Some("current"),
            "claims --json must only surface current claims: {claim}"
        );
        assert!(
            claim["receipt"]["sequence"].as_u64().unwrap_or(0) > 0,
            "each claim must carry a non-zero receipt sequence: {claim}"
        );
    }
    let old_id = claims_arr[0]["claim_id"]
        .as_str()
        .expect("claim_id")
        .to_string();
    let new_id = claims_arr[1]["claim_id"]
        .as_str()
        .expect("claim_id")
        .to_string();
    assert_ne!(old_id, new_id, "fixture must yield >=2 distinct claims");
    (old_id, new_id)
}

/// agent-context with and without flags: default prints JSON to stdout; `--out`
/// writes a file and prints nothing.
fn phase_agent_context(root: &Path) {
    let default_out = texo(root)
        .args(["agent-context"])
        .output()
        .expect("agent-context");
    assert!(default_out.status.success());
    let ctx: Value = serde_json::from_slice(&default_out.stdout)
        .expect("agent-context with no flags must print JSON to stdout");
    assert_eq!(
        ctx["workspace_id"].as_str(),
        Some("demo"),
        "agent-context must report the workspace id"
    );
    assert!(
        ctx["replayed_through_sequence"].as_u64().unwrap_or(0) > 0,
        "agent-context frontier must advance past zero: {ctx}"
    );
    assert!(
        ctx["claims"].as_array().is_some_and(|c| !c.is_empty()),
        "agent-context must carry a non-empty claims array"
    );

    let out_only = texo(root)
        .args(["agent-context", "--out", "public/agent-context.json"])
        .output()
        .expect("agent-context --out");
    assert!(out_only.status.success());
    assert!(
        out_only.stdout.is_empty(),
        "agent-context --out (no --json) must not print to stdout"
    );
    let written = root.join("public/agent-context.json");
    assert!(written.is_file(), "--out must write the context file");
    let on_disk: Value =
        serde_json::from_slice(&std::fs::read(&written).expect("read")).expect("file JSON");
    assert_eq!(
        on_disk["workspace_id"].as_str(),
        Some("demo"),
        "file content must match the rendered context"
    );
}

/// check-staleness --json on a known-stale doc must flag a superseded claim.
fn phase_check_staleness(root: &Path) {
    let stale = json_stdout(texo(root).args([
        "check-staleness",
        "sample_sources/stale_onboarding.md",
        "--json",
    ]));
    let diagnostics = stale["diagnostics"]
        .as_array()
        .expect("check-staleness JSON must carry a diagnostics array");
    assert!(
        !diagnostics.is_empty(),
        "the stale onboarding doc must yield staleness diagnostics: {stale}"
    );
    assert!(
        diagnostics.iter().any(|d| {
            d["superseded_by"].as_str().is_some()
                && d["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("superseded"))
        }),
        "stale onboarding must flag a claim as superseded (superseded_by + message): {stale}"
    );
}

/// compile must write every expected artifact under the out dir.
fn phase_compile(root: &Path) {
    texo(root)
        .args(["compile", "--out", "public"])
        .assert()
        .success();
    for name in [
        "claims.json",
        "agent-context.json",
        "conflicts.json",
        "stale-context.json",
        "onboarding.generated.md",
        "index.html",
    ] {
        assert!(
            root.join("public").join(name).is_file(),
            "compile must write public/{name}"
        );
    }
}

/// conflicts read + --commit branches. Sample sources self-resolve via
/// auto-supersession, so both report an empty set with a well-formed shape.
fn phase_conflicts(root: &Path) {
    let conflicts = json_stdout(texo(root).args(["conflicts", "--json"]));
    assert_eq!(
        conflicts["workspace_id"].as_str(),
        Some("demo"),
        "conflicts report must name the workspace"
    );
    assert_eq!(
        conflicts["conflicts"].as_array().map(Vec::len),
        Some(0),
        "auto-superseded sample sources must leave zero open conflicts: {conflicts}"
    );

    // Exercises the distinct commit code path (commit_conflicts + journal
    // append). With no open conflicts it commits an empty set.
    let committed = json_stdout(texo(root).args(["conflicts", "--commit", "--json"]));
    assert_eq!(
        committed.as_array().map(Vec::len),
        Some(0),
        "committing with no open conflicts yields an empty receipt list: {committed}"
    );
}

/// verify --json must report a clean projection + journal and return the
/// current frontier so the supersede phase can assert it advances.
fn phase_verify(root: &Path) -> u64 {
    let verify = json_stdout(texo(root).args(["verify", "--json"]));
    assert_eq!(
        verify["projection_ok"].as_bool(),
        Some(true),
        "projection must verify clean: {verify}"
    );
    assert_eq!(
        verify["journal_ok"].as_bool(),
        Some(true),
        "journal receipts must verify clean: {verify}"
    );
    assert_eq!(
        verify["errors"].as_array().map(Vec::len),
        Some(0),
        "clean verify must report no errors: {verify}"
    );
    let frontier = verify["replayed_through_sequence"]
        .as_u64()
        .expect("frontier");
    assert!(frontier > 0, "verify frontier must be non-zero");
    frontier
}

/// supersede happy path (drops the old claim from the current set, appends past
/// the prior frontier) plus the invalid-id error path (non-zero exit, no panic).
fn phase_supersede(root: &Path, old_id: &str, new_id: &str, frontier_before: u64) {
    let receipt = json_stdout(texo(root).args([
        "supersede",
        old_id,
        new_id,
        "--reason",
        "e2e manual supersede",
        "--decided-by",
        "human",
        "--json",
    ]));
    assert_eq!(
        receipt["kind"].as_str(),
        Some("ClaimSuperseded"),
        "supersede must emit a ClaimSuperseded receipt: {receipt}"
    );
    let sup_seq = receipt["sequence"].as_u64().expect("supersede sequence");
    assert!(
        sup_seq > frontier_before,
        "supersede must append past the prior frontier ({sup_seq} > {frontier_before})"
    );

    let after = json_stdout(texo(root).args(["claims", "--json"]));
    let after_ids: Vec<&str> = after
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|c| c["claim_id"].as_str())
        .collect();
    assert!(
        !after_ids.contains(&old_id),
        "superseded claim {old_id} must drop out of the current set: {after_ids:?}"
    );

    // Argument parses past clap but fails ClaimId validation in core; the
    // command must exit non-zero rather than panic.
    texo(root)
        .args([
            "supersede",
            "not-a-claim-id",
            "also-not-valid",
            "--reason",
            "bad",
        ])
        .assert()
        .failure();
}

#[test]
fn full_pipeline_smoke() {
    let dir: TempDir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    seed_sources(root);

    phase_init_and_ingest(root);
    let (old_id, new_id) = phase_claims(root);
    phase_agent_context(root);
    phase_check_staleness(root);
    phase_compile(root);
    phase_conflicts(root);
    let frontier_before = phase_verify(root);
    phase_supersede(root, &old_id, &new_id, frontier_before);
}
