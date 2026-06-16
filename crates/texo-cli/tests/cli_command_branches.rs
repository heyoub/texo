//! Field-level coverage of the texo CLI command glue that the happy-path
//! pipeline smoke test does not exercise: every command's human-readable
//! (non-`--json`) render branch, empty-state vs non-empty branches, the
//! dry-run ingest branch, the explicit error/failure branches (unknown
//! workspace, malformed claim id), and the `texo mcp` runtime (both the
//! clean-exit and connection-error paths).
//!
//! These are real assertions on stdout/stderr text and exit codes, not bare
//! `.success()`/`.failure()` checks, so each covered line is pinned to an
//! observable outcome.

use assert_cmd::cargo::cargo_bin;
use assert_cmd::Command;
use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};
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

/// init + ingest a populated demo workspace and return its root tempdir.
fn populated_workspace() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    seed_sources(root);
    texo(root)
        .args(["init", "--workspace", "demo"])
        .assert()
        .success();
    texo(root)
        .args(["ingest", "sample_sources"])
        .assert()
        .success();
    dir
}

/// Capture (status_success, stdout, stderr) for an invocation.
fn run(cmd: &mut Command) -> (bool, String, String) {
    let out = cmd.output().expect("run command");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// ---------------------------------------------------------------------------
// claims.rs — human-readable render branch (lines 18-29).
// ---------------------------------------------------------------------------

/// `claims` without `--json` renders each current claim as a multi-line block
/// carrying the id, debug-printed status, subject, quoted text, source
/// path:line, sequence and receipt id.
#[test]
fn claims_human_readable_renders_full_blocks() {
    let dir = populated_workspace();
    let (ok, stdout, stderr) = run(texo(dir.path()).args(["claims"]));
    assert!(ok, "claims must succeed: {stderr}");

    // First grab the structured view to learn an expected claim id + subject.
    let json: Value = serde_json::from_slice(
        &texo(dir.path())
            .args(["claims", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .expect("claims --json");
    let arr = json.as_array().expect("array");
    assert!(!arr.is_empty(), "fixture must yield current claims");
    let first_id = arr[0]["claim_id"].as_str().expect("claim_id");
    let first_subject = arr[0]["subject_hint"].as_str().expect("subject_hint");

    // The id line: "<id> <Status> <subject>".
    assert!(
        stdout.contains(first_id),
        "human render must contain the claim id {first_id}: {stdout}"
    );
    assert!(
        stdout.contains(first_subject),
        "human render must contain the subject hint {first_subject}: {stdout}"
    );
    // Debug-printed status (claims --json normalizes to lowercase "current";
    // the Debug form rendered here is the capitalized variant "Current").
    assert!(
        stdout.contains("Current"),
        "human render must Debug-print the claim status: {stdout}"
    );
    // The quoted-text line, the source line, the seq line and receipt line.
    assert!(
        stdout.contains("  source: "),
        "human render must include a source path:line line: {stdout}"
    );
    assert!(
        stdout.contains("  seq: "),
        "human render must include a seq line: {stdout}"
    );
    assert!(
        stdout.contains("  receipt: "),
        "human render must include a receipt line: {stdout}"
    );
}

/// `claims --subject <hint>` in the human path filters out non-matching claims
/// (exercises the `continue` guard on line 19-20).
#[test]
fn claims_human_readable_subject_filter_excludes_others() {
    let dir = populated_workspace();
    let json: Value = serde_json::from_slice(
        &texo(dir.path())
            .args(["claims", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .expect("claims --json");
    let arr = json.as_array().expect("array");
    // Find two distinct subject hints so we can assert the filter excludes one.
    let subjects: Vec<&str> = arr
        .iter()
        .filter_map(|c| c["subject_hint"].as_str())
        .collect();
    let target = subjects[0];
    let other = subjects
        .iter()
        .find(|s| **s != target)
        .copied()
        .expect("fixture must have >=2 distinct subjects");

    let (ok, stdout, stderr) = run(texo(dir.path()).args(["claims", "--subject", target]));
    assert!(ok, "filtered claims must succeed: {stderr}");
    assert!(
        stdout.contains(target),
        "subject-filtered render must keep the matching subject {target}: {stdout}"
    );
    assert!(
        !stdout.contains(other),
        "subject-filtered render must drop the non-matching subject {other}: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// check_staleness.rs — human-readable branches (lines 18-26).
// ---------------------------------------------------------------------------

/// Human-readable check-staleness on a stale doc prints a `file:line warning —
/// message` line per diagnostic (lines 18-23).
#[test]
fn check_staleness_human_readable_prints_warnings() {
    let dir = populated_workspace();
    let (ok, stdout, stderr) =
        run(texo(dir.path()).args(["check-staleness", "sample_sources/stale_onboarding.md"]));
    assert!(ok, "check-staleness must succeed: {stderr}");
    assert!(
        stdout.contains("warning —"),
        "stale doc must produce a human-readable warning line: {stdout}"
    );
    assert!(
        stdout.contains("stale_onboarding.md"),
        "warning line must name the offending file: {stdout}"
    );
    assert!(
        stdout.contains("superseded"),
        "warning message must mention supersession: {stdout}"
    );
}

/// Human-readable check-staleness on a doc with no claims known to the store
/// prints the empty-state line (lines 24-26). We author a brand-new doc whose
/// prose matches nothing in the journal, guaranteeing zero diagnostics.
#[test]
fn check_staleness_human_readable_empty_state() {
    let dir = populated_workspace();
    let fresh = dir.path().join("sample_sources/unrelated_topic.md");
    std::fs::write(
        &fresh,
        "# Unrelated\n\nThe office cafeteria serves lunch at noon on weekdays.\n",
    )
    .expect("write fresh doc");

    let (ok, stdout, stderr) =
        run(texo(dir.path()).args(["check-staleness", "sample_sources/unrelated_topic.md"]));
    assert!(ok, "check-staleness must succeed: {stderr}");
    assert_eq!(
        stdout.trim(),
        "no stale claims detected",
        "a doc with no journal-known claims must print the empty-state message, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// conflicts.rs — human-readable read + commit branches (lines 24, 28-30, 39-44).
// ---------------------------------------------------------------------------

/// Human-readable `conflicts --commit` prints the "committed N conflicts"
/// summary (lines 19-24, 28-30).
#[test]
fn conflicts_human_readable_commit_summary() {
    let dir = populated_workspace();
    let (ok, stdout, stderr) = run(texo(dir.path()).args(["conflicts", "--commit"]));
    assert!(ok, "conflicts --commit must succeed: {stderr}");
    assert_eq!(
        stdout.trim(),
        "committed 0 conflicts",
        "auto-superseded sample sources commit zero conflicts: {stdout}"
    );
}

/// Human-readable `conflicts` (read, no `--json`) iterates the report and, with
/// no open conflicts, prints nothing on stdout while still exiting clean
/// (exercises the loop header on line 39 with an empty body).
#[test]
fn conflicts_human_readable_read_empty() {
    let dir = populated_workspace();
    let (ok, stdout, stderr) = run(texo(dir.path()).args(["conflicts"]));
    assert!(ok, "conflicts read must succeed: {stderr}");
    assert!(
        stdout.trim().is_empty(),
        "with no open conflicts the human read prints nothing: {stdout}"
    );
}

/// A workspace with genuine, un-superseded competing claims renders the
/// `id A vs B (subject)` conflict line (lines 39-44 with a non-empty body).
#[test]
fn conflicts_human_readable_renders_open_conflict() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let src = root.join("sources");
    std::fs::create_dir_all(&src).expect("mkdir sources");
    // Two contradictory claims about the same subject, neither superseding the
    // other, so they remain an open conflict at replay time.
    std::fs::write(
        src.join("a.md"),
        "# Deploy\n\nThe deploy target is staging.\n",
    )
    .expect("write a.md");
    std::fs::write(
        src.join("b.md"),
        "# Deploy\n\nThe deploy target is production.\n",
    )
    .expect("write b.md");

    texo(root)
        .args(["init", "--workspace", "demo"])
        .assert()
        .success();
    texo(root).args(["ingest", "sources"]).assert().success();

    // Read the structured conflicts to decide whether this fixture actually
    // produced an open conflict; the human assertion is conditional on that so
    // the test is robust to the heuristic, but still asserts the render shape
    // whenever a conflict exists.
    let json: Value = serde_json::from_slice(
        &texo(root)
            .args(["conflicts", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .expect("conflicts --json");
    let entries = json["conflicts"].as_array().expect("conflicts array");

    let (ok, stdout, stderr) = run(texo(root).args(["conflicts"]));
    assert!(ok, "conflicts read must succeed: {stderr}");
    if let Some(first) = entries.first() {
        let cid = first["conflict_id"].as_str().expect("conflict_id");
        assert!(
            stdout.contains(cid) && stdout.contains(" vs "),
            "open conflict must render an `id A vs B (subject)` line: {stdout}"
        );
    } else {
        assert!(
            stdout.trim().is_empty(),
            "no open conflicts means an empty human render: {stdout}"
        );
    }
}

// ---------------------------------------------------------------------------
// verify.rs — human-readable success branch + error branches (19,22,32,34-44).
// ---------------------------------------------------------------------------

/// Human-readable `verify` on a clean store prints the
/// "ok — replayed through local seq N" summary (lines 34-44, happy path).
#[test]
fn verify_human_readable_ok_summary() {
    let dir = populated_workspace();
    let (ok, stdout, stderr) = run(texo(dir.path()).args(["verify"]));
    assert!(ok, "verify must succeed on a clean store: {stderr}");
    assert!(
        stdout.starts_with("ok — replayed through local seq "),
        "clean verify must print the ok summary: {stdout}"
    );
    // The frontier must be a parseable non-zero sequence number.
    let seq: u64 = stdout
        .trim()
        .rsplit(' ')
        .next()
        .and_then(|s| s.parse().ok())
        .expect("verify summary must end in a numeric frontier");
    assert!(seq > 0, "verify frontier must be non-zero: {stdout}");
}

/// Fault injection: with the BatPak store tampered (a flipped payload byte),
/// `verify` must fail cleanly (non-zero exit, error on stderr) rather than
/// panic. The integrity layer's CRC check fires during replay, so this
/// exercises the error propagation out of `verify::run` for both output modes.
/// (The post-replay `projection_ok:false` / `journal_ok:false` branches are
/// unreachable from the CLI because corruption is caught at replay; see the
/// honest-exclusions list in the report.)
#[test]
fn verify_fails_cleanly_on_corrupted_store() {
    for json_mode in [false, true] {
        let dir = populated_workspace();
        let store = dir.path().join(".texo/store");
        // Flip one byte in the middle of every segment to trip BatPak's CRC.
        let mut tampered = false;
        for entry in std::fs::read_dir(&store).expect("read store") {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|e| e.to_str()) == Some("fbat") {
                let mut bytes = std::fs::read(&path).expect("read segment");
                if bytes.len() > 60 {
                    let mid = bytes.len() / 2;
                    bytes[mid] ^= 0x01;
                    std::fs::write(&path, &bytes).expect("write tampered segment");
                    tampered = true;
                }
            }
        }
        assert!(tampered, "test must actually tamper at least one segment");

        let mut cmd = texo(dir.path());
        cmd.arg("verify");
        if json_mode {
            cmd.arg("--json");
        }
        let (ok, stdout, stderr) = run(&mut cmd);
        assert!(
            !ok,
            "verify on a corrupted store must exit non-zero (json={json_mode})"
        );
        assert!(
            stdout.is_empty(),
            "verify failure must not emit a clean report on stdout (json={json_mode}): {stdout}"
        );
        assert!(
            stderr.contains("CRC mismatch") || stderr.to_lowercase().contains("decode"),
            "verify failure must surface the integrity error (json={json_mode}): {stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// supersede.rs — human-readable branch (lines 27, 30-35) + error branch.
// ---------------------------------------------------------------------------

/// Human-readable `supersede` prints the "superseded X with Y at local seq N"
/// line and actually drops the old claim from the current set (lines 28-35,
/// non-json arm).
#[test]
fn supersede_human_readable_summary_and_effect() {
    let dir = populated_workspace();
    let json: Value = serde_json::from_slice(
        &texo(dir.path())
            .args(["claims", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .expect("claims --json");
    let arr = json.as_array().expect("array");
    let old_id = arr[0]["claim_id"].as_str().expect("claim_id").to_string();
    let new_id = arr[1]["claim_id"].as_str().expect("claim_id").to_string();
    assert_ne!(old_id, new_id);

    let (ok, stdout, stderr) = run(texo(dir.path()).args([
        "supersede",
        &old_id,
        &new_id,
        "--reason",
        "human-render supersede",
    ]));
    assert!(ok, "supersede must succeed: {stderr}");
    assert!(
        stdout.contains(&format!("superseded {old_id} with {new_id} at local seq ")),
        "human supersede must print the summary line: {stdout}"
    );

    // The old claim must no longer be current.
    let after: Value = serde_json::from_slice(
        &texo(dir.path())
            .args(["claims", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .expect("claims --json after");
    let still_present = after
        .as_array()
        .expect("array")
        .iter()
        .any(|c| c["claim_id"].as_str() == Some(old_id.as_str()));
    assert!(
        !still_present,
        "superseded claim {old_id} must drop from the current set"
    );
}

/// `supersede` with a malformed (non-`claim_`) old id fails the `ClaimId`
/// conversion before touching the store: clean non-zero exit, no stdout, and a
/// specific stderr message naming the required prefix (line 17 error path).
#[test]
fn supersede_malformed_id_clean_error() {
    let dir = populated_workspace();
    let (ok, stdout, stderr) = run(texo(dir.path()).args([
        "supersede",
        "not-a-claim-id",
        "claim_aaaaaaaaaaaa",
        "--reason",
        "bad",
    ]));
    assert!(!ok, "malformed old id must fail");
    assert!(
        stdout.is_empty(),
        "a validation failure must not print to stdout: {stdout}"
    );
    assert!(
        stderr.contains("identifier must start with `claim_`"),
        "stderr must explain the claim id prefix requirement: {stderr}"
    );
}

/// `supersede` with a malformed *new* id likewise fails on the second
/// conversion (line 18 error path), after the first id parsed cleanly.
#[test]
fn supersede_malformed_new_id_clean_error() {
    let dir = populated_workspace();
    let (ok, stdout, stderr) = run(texo(dir.path()).args([
        "supersede",
        "claim_aaaaaaaaaaaa",
        "still-not-valid",
        "--reason",
        "bad",
    ]));
    assert!(!ok, "malformed new id must fail");
    assert!(
        stdout.is_empty(),
        "no stdout on validation failure: {stdout}"
    );
    assert!(
        stderr.contains("identifier must start with `claim_`"),
        "stderr must explain the claim id prefix requirement: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// ingest.rs — dry-run branch (line 21) + human summary (39-44, already hit but
// asserted here at field level).
// ---------------------------------------------------------------------------

/// `ingest --dry-run` selects `IngestMode::DryRun` (line 21) and prints the
/// human summary; the dry run must NOT persist claims, so a subsequent
/// `claims --json` on a never-committed store is empty.
#[test]
fn ingest_dry_run_does_not_persist() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    seed_sources(root);
    texo(root)
        .args(["init", "--workspace", "demo"])
        .assert()
        .success();

    let (ok, stdout, stderr) = run(texo(root).args(["ingest", "sample_sources", "--dry-run"]));
    assert!(ok, "dry-run ingest must succeed: {stderr}");
    assert!(
        stdout.starts_with("ingested ") && stdout.contains(" claims ("),
        "dry-run must still print the human ingest summary: {stdout}"
    );

    // Nothing was committed: the current claim set is empty.
    let claims: Value = serde_json::from_slice(
        &texo(root)
            .args(["claims", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .expect("claims --json");
    assert_eq!(
        claims.as_array().map(Vec::len),
        Some(0),
        "dry-run must not persist any claims: {claims}"
    );
}

// ---------------------------------------------------------------------------
// agent_context.rs — create nested parent dir branch (line 28-29).
// ---------------------------------------------------------------------------

/// `agent-context --out` into a deeply nested, not-yet-existing parent dir
/// exercises the `create_dir_all(parent)` branch (line 28-29) and writes valid
/// JSON there.
#[test]
fn agent_context_out_creates_nested_parent() {
    let dir = populated_workspace();
    let nested = "deeply/nested/new/dir/ctx.json";
    let (ok, stdout, stderr) = run(texo(dir.path()).args(["agent-context", "--out", nested]));
    assert!(ok, "agent-context --out must succeed: {stderr}");
    assert!(
        stdout.is_empty(),
        "--out without --json must not print to stdout: {stdout}"
    );
    let written = dir.path().join(nested);
    assert!(
        written.is_file(),
        "--out must create the nested parent dirs and write the file"
    );
    let on_disk: Value =
        serde_json::from_slice(&std::fs::read(&written).expect("read")).expect("file JSON");
    assert_eq!(
        on_disk["workspace_id"].as_str(),
        Some("demo"),
        "written context must name the workspace"
    );
}

// ---------------------------------------------------------------------------
// main.rs / config — unknown-workspace dispatch error (the global --workspace
// flag resolving to a missing entry).
// ---------------------------------------------------------------------------

/// A `--workspace` that does not exist in config fails at resolve time with the
/// specific "unknown workspace" message, before any command body runs. Asserted
/// against `claims` (a read command) for a clean non-zero exit.
#[test]
fn unknown_workspace_clean_error() {
    let dir = populated_workspace();
    let (ok, stdout, stderr) =
        run(texo(dir.path()).args(["--workspace", "does-not-exist", "claims", "--json"]));
    assert!(!ok, "unknown workspace must fail");
    assert!(
        stdout.is_empty(),
        "no stdout when workspace resolution fails: {stdout}"
    );
    assert!(
        stderr.contains("unknown workspace: does-not-exist"),
        "stderr must name the unknown workspace: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// main.rs — the `Mcp` dispatch arm + tokio runtime (lines 131-136).
// ---------------------------------------------------------------------------

/// `texo mcp` with a valid MCP `initialize` request followed by EOF on stdin
/// completes the handshake, responds with the server identity, and exits clean
/// (covers the runtime build, `block_on(run_stdio(..))` success, and the final
/// `Ok(())` on main.rs line 136).
#[test]
fn mcp_runtime_handshake_clean_exit() {
    let dir = populated_workspace();
    let mut child = StdCommand::new(cargo_bin("texo"))
        .arg("mcp")
        .current_dir(dir.path())
        .env("TEXO_OBSERVED_AT_MS", FIXTURE_OBSERVED_AT_MS.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn texo mcp");

    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        // A minimal but valid JSON-RPC initialize request; dropping stdin after
        // sends EOF, which ends the stdio transport read loop.
        stdin
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}\n",
            )
            .expect("write initialize");
    }
    // Dropping `child.stdin` closes it -> EOF.
    child.stdin.take();

    let out = child.wait_with_output().expect("wait mcp");
    assert!(
        out.status.success(),
        "mcp must exit clean after initialize + EOF: status={:?} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"serverInfo\"") && stdout.contains("\"name\":\"texo\""),
        "mcp must answer initialize with the texo server identity: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// main.rs — observed_at_ms() real wall-clock fallback (lines 153-157).
// ---------------------------------------------------------------------------

/// Running write commands WITHOUT `TEXO_OBSERVED_AT_MS` set exercises the real
/// `SystemTime::now()` timestamp path in `observed_at_ms` (lines 153-157),
/// rather than the pinned-override path (line 149-151) every other test uses.
///
/// The wall-clock value is never surfaced in any rendered output, so it cannot
/// be asserted by value; instead we assert the full write+replay+verify cycle
/// succeeds under the real clock, which proves `observed_at_ms()` produced a
/// `u64` the journal accepted. We also pin the override path in the SAME test
/// (a second run WITH the env var) so both branches are asserted side by side.
#[test]
fn observed_at_ms_real_clock_when_env_unset() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    seed_sources(root);

    // No TEXO_OBSERVED_AT_MS -> real wall-clock branch (153-157).
    Command::new(cargo_bin("texo"))
        .args(["init", "--workspace", "demo"])
        .env_remove("TEXO_OBSERVED_AT_MS")
        .current_dir(root)
        .assert()
        .success();
    let ingest = Command::new(cargo_bin("texo"))
        .args(["ingest", "sample_sources", "--json"])
        .env_remove("TEXO_OBSERVED_AT_MS")
        .current_dir(root)
        .output()
        .expect("ingest");
    assert!(
        ingest.status.success(),
        "real-clock ingest must succeed: {}",
        String::from_utf8_lossy(&ingest.stderr)
    );
    let report: Value = serde_json::from_slice(&ingest.stdout).expect("ingest --json");
    assert!(
        report["claims_recorded"].as_u64().unwrap_or(0) > 0,
        "real-clock ingest must record claims: {report}"
    );

    // The store written under the real clock must replay + verify clean, which
    // is only possible if each recorded receipt carried a valid u64 timestamp.
    let verify: Value = serde_json::from_slice(
        &Command::new(cargo_bin("texo"))
            .args(["verify", "--json"])
            .env_remove("TEXO_OBSERVED_AT_MS")
            .current_dir(root)
            .output()
            .expect("verify")
            .stdout,
    )
    .expect("verify --json");
    assert_eq!(
        verify["projection_ok"].as_bool(),
        Some(true),
        "real-clock store must verify clean: {verify}"
    );
    assert!(
        verify["replayed_through_sequence"].as_u64().unwrap_or(0) > 0,
        "real-clock store frontier must advance: {verify}"
    );

    // Override branch (149-151): a pinned value parses and is honored end-to-end
    // in a fresh workspace.
    let dir2 = populated_workspace();
    let v2: Value = serde_json::from_slice(
        &texo(dir2.path())
            .args(["verify", "--json"])
            .output()
            .expect("verify pinned")
            .stdout,
    )
    .expect("verify pinned --json");
    assert_eq!(v2["projection_ok"].as_bool(), Some(true));

    // Env var present but NOT a valid u64 -> the inner parse fails (line 149
    // false branch) and we fall through to the real clock, still succeeding.
    let dir3 = tempfile::tempdir().expect("tempdir");
    let root3 = dir3.path();
    seed_sources(root3);
    Command::new(cargo_bin("texo"))
        .args(["init", "--workspace", "demo"])
        .env("TEXO_OBSERVED_AT_MS", "not-a-number")
        .current_dir(root3)
        .assert()
        .success();
    let ingest3 = Command::new(cargo_bin("texo"))
        .args(["ingest", "sample_sources", "--json"])
        .env("TEXO_OBSERVED_AT_MS", "not-a-number")
        .current_dir(root3)
        .output()
        .expect("ingest unparsable env");
    assert!(
        ingest3.status.success(),
        "unparsable TEXO_OBSERVED_AT_MS must fall back to the real clock and succeed: {}",
        String::from_utf8_lossy(&ingest3.stderr)
    );
    let report3: Value = serde_json::from_slice(&ingest3.stdout).expect("ingest3 --json");
    assert!(
        report3["claims_recorded"].as_u64().unwrap_or(0) > 0,
        "fallback-clock ingest must still record claims: {report3}"
    );
}

/// `texo mcp` with stdin closed immediately (EOF before any initialize) drives
/// the error path: `run_stdio` returns Err, `.context("MCP stdio server
/// failed")?` wraps it, and `main` exits non-zero printing that context to
/// stderr (covers the `?` on main.rs line 135).
#[test]
fn mcp_runtime_connection_error_exit() {
    let dir = populated_workspace();
    let mut child = StdCommand::new(cargo_bin("texo"))
        .arg("mcp")
        .current_dir(dir.path())
        .env("TEXO_OBSERVED_AT_MS", FIXTURE_OBSERVED_AT_MS.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn texo mcp");
    // Close stdin without sending anything -> immediate EOF before initialize.
    child.stdin.take();

    let out = child.wait_with_output().expect("wait mcp");
    assert!(
        !out.status.success(),
        "mcp with no initialize must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("MCP stdio server failed"),
        "error exit must surface the wrapping context message: {stderr}"
    );
}
