//! External command extractor adapter.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use serde::Deserialize;

use crate::events::payloads::ClaimRecorded;
use crate::source::markdown::MarkdownDocument;
use crate::types::ids::{claim_id_from_parts, SourceId};

use super::{ExtractError, ExtractedClaim};

/// Maximum wall-clock runtime allowed for an external extractor before it is killed.
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(30);

/// Poll interval used while waiting for the child to exit. Short enough that the
/// timeout fires promptly, long enough not to busy-spin.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Inclusive upper bound for `confidence_ppm` (parts-per-million).
const MAX_CONFIDENCE_PPM: u32 = 1_000_000;

/// Minimal JSON line shape from an external extractor command.
#[derive(Debug, Deserialize)]
struct CmdClaimLine {
    line_start: u32,
    text: String,
    normalized_text: Option<String>,
    subject_hint: Option<String>,
    predicate_hint: Option<String>,
    object_hint: Option<String>,
    confidence_ppm: Option<u32>,
}

/// Run an external extractor command and parse newline-delimited JSON claims.
pub fn extract_via_cmd(
    cmd: &str,
    doc: &MarkdownDocument,
    source_id: &SourceId,
    workspace_id: &str,
    observed_at_ms: u64,
    root: &Path,
) -> Result<Vec<ExtractedClaim>, ExtractError> {
    extract_via_cmd_with_timeout(
        cmd,
        doc,
        source_id,
        workspace_id,
        observed_at_ms,
        root,
        EXTRACT_TIMEOUT,
    )
}

/// Run an external extractor command with an explicit timeout.
///
/// `child` is owned by this (the main) thread. Dedicated reader threads drain
/// stdout and stderr so neither pipe can deadlock a child that fills its buffer.
/// The deadline is enforced by polling `child.try_wait()`: if the child overruns
/// it is killed and reaped, which closes the pipes so the reader threads reach
/// EOF and join cleanly.
fn extract_via_cmd_with_timeout(
    cmd: &str,
    doc: &MarkdownDocument,
    source_id: &SourceId,
    workspace_id: &str,
    observed_at_ms: u64,
    root: &Path,
    timeout: Duration,
) -> Result<Vec<ExtractedClaim>, ExtractError> {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(format!("{cmd} \"$1\""))
        .arg("texo-extract")
        .arg(&doc.path)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Run the extractor in its own process group so a timeout can kill the whole
    // tree, not just the immediate `sh`. Otherwise a grandchild (e.g. `sh -c`
    // spawning `sleep`) keeps the stdout pipe open after we kill `sh`, and the
    // reader thread blocks until the grandchild eventually exits.
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn().map_err(|e| ExtractError::CmdIo {
        context: "spawn failed",
        source: e,
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ExtractError::Cmd("missing stdout".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ExtractError::Cmd("missing stderr".to_string()))?;

    // Read both pipes on dedicated threads. If either pipe's ~64KB buffer fills
    // while we only read the other, the child blocks forever; draining both
    // concurrently avoids that deadlock. Each thread returns its captured bytes.
    let stdout_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let mut reader = stdout;
        let _ = reader.read_to_end(&mut buf);
        buf
    });
    let stderr_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let mut reader = stderr;
        // Ignore read errors; we only use captured stderr for diagnostics.
        let _ = reader.read_to_end(&mut buf);
        buf
    });

    // Enforce the deadline by polling. Killing the child on overrun closes its
    // pipes, letting the reader threads reach EOF so we can join them below.
    let status = match wait_with_timeout(&mut child, timeout) {
        Ok(status) => status,
        Err(e) => {
            // Child already killed+reaped by wait_with_timeout. On Unix the
            // process-group SIGKILL closed the pipes (incl. grandchildren), so
            // these joins return promptly. On non-Unix the fallback kills only
            // the immediate child; a grandchild holding the pipe could keep a
            // reader thread blocked here — see `kill_tree`. The extractor always
            // spawns `sh -c`, which is unix-only, so that path is unreachable in
            // practice (spawn fails first on non-Unix).
            let _ = collect_bytes(stdout_handle);
            let stderr_text = collect_stderr(stderr_handle);
            return Err(append_stderr(e, &stderr_text));
        }
    };

    let basename = Path::new(cmd)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("cmd");
    let extractor_kind = format!("cmd:{basename}");

    // Upper bound for line_start/line_end is the RAW file line domain, not the
    // length of the FILTERED `doc.lines` (which drops frontmatter and fenced
    // code-block lines). `line.number` is the raw 1-based file line number, the
    // same domain the heuristic extractor and scripts/extract-identity.py use.
    let max_line = max_raw_line(doc);

    let stdout_bytes = collect_bytes(stdout_handle);
    let stderr_text = collect_stderr(stderr_handle);

    let parse_result = parse_claims(
        &stdout_bytes,
        max_line,
        doc,
        source_id,
        workspace_id,
        observed_at_ms,
        &extractor_kind,
    );

    let claims = match parse_result {
        Ok(claims) => claims,
        Err(e) => return Err(append_stderr(e, &stderr_text)),
    };

    if !status.success() {
        return Err(ExtractError::Cmd(format!(
            "extractor exited with {status}{}",
            stderr_suffix(&stderr_text)
        )));
    }

    Ok(claims)
}

/// Parse newline-delimited JSON stdout into validated claims.
fn parse_claims(
    stdout_bytes: &[u8],
    max_line: u32,
    doc: &MarkdownDocument,
    source_id: &SourceId,
    workspace_id: &str,
    observed_at_ms: u64,
    extractor_kind: &str,
) -> Result<Vec<ExtractedClaim>, ExtractError> {
    let text = std::str::from_utf8(stdout_bytes)?;

    let mut claims = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: CmdClaimLine = serde_json::from_str(trimmed)?;
        let claim = build_claim(
            parsed,
            max_line,
            doc,
            source_id,
            workspace_id,
            observed_at_ms,
            extractor_kind,
        )?;
        claims.push(claim);
    }
    Ok(claims)
}

/// Largest raw 1-based file line number represented in the parsed document.
///
/// `parse_lines` drops frontmatter and fenced code-block lines but preserves the
/// raw `number` of every retained line, so the maximum retained `number` is the
/// correct in-range upper bound for an extractor's raw `line_start`.
fn max_raw_line(doc: &MarkdownDocument) -> u32 {
    doc.lines.iter().map(|l| l.number).max().unwrap_or(0)
}

/// Wait for `child` to exit within `timeout` by polling `try_wait`.
///
/// std has no wait-with-timeout. We keep ownership of the child here and poll so
/// we can kill a hung child even if it is holding a pipe open. On timeout the
/// child is killed and reaped and a timeout error is returned.
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Result<ExitStatus, ExtractError> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_tree(child);
                    let _ = child.wait();
                    return Err(ExtractError::Cmd(format!(
                        "extractor timed out after {}ms",
                        timeout.as_millis()
                    )));
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(e) => {
                kill_tree(child);
                let _ = child.wait();
                return Err(ExtractError::CmdIo {
                    context: "wait failed",
                    source: e,
                });
            }
        }
    }
}

/// Kill the extractor and any descendants it spawned.
///
/// On Unix the child is its own process-group leader (its PGID equals its PID),
/// so we SIGKILL the whole group to reap grandchildren that may still be holding
/// the stdout/stderr pipes open. Killing only `child` would leave such a
/// grandchild alive, keeping a pipe open and blocking the reader threads until
/// it exits on its own. We then also kill `child` directly so it is reaped via
/// `child.wait()`.
///
/// Limitation: the process-group reap is Unix-only. On non-Unix the fallback
/// kills just the immediate child, so a grandchild could keep a pipe open and
/// block a reader thread. This is acceptable because the extractor always
/// spawns via `sh -c` (unix-only); on non-Unix the command fails to spawn
/// before this path is reached.
fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        // PGID == leader PID; the negative pid targets the whole group.
        let pgid = child.id();
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg(format!("-{pgid}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

/// Validate one parsed JSON line and build the corresponding claim.
///
/// Rejects out-of-range `line_start` (must be in `1..=max_line`, since file lines
/// are 1-based) and out-of-range `confidence_ppm` (must be in `0..=1_000_000`).
fn build_claim(
    parsed: CmdClaimLine,
    max_line: u32,
    doc: &MarkdownDocument,
    source_id: &SourceId,
    workspace_id: &str,
    observed_at_ms: u64,
    extractor_kind: &str,
) -> Result<ExtractedClaim, ExtractError> {
    // Validate line_start against the RAW file line domain.
    if parsed.line_start == 0 || parsed.line_start > max_line {
        return Err(ExtractError::Cmd(format!(
            "line_start {} out of range (file has {} lines)",
            parsed.line_start, max_line
        )));
    }

    // Validate confidence_ppm into 0..=1_000_000 when present.
    let confidence_ppm = match parsed.confidence_ppm {
        Some(ppm) if ppm > MAX_CONFIDENCE_PPM => {
            return Err(ExtractError::Cmd(format!(
                "confidence_ppm {ppm} out of range (max {MAX_CONFIDENCE_PPM})"
            )));
        }
        Some(ppm) => ppm,
        None => super::DEFAULT_CONFIDENCE_PPM,
    };

    let normalized = parsed
        .normalized_text
        .unwrap_or_else(|| super::normalize::normalize_line(&parsed.text));
    let claim_id = claim_id_from_parts(source_id, parsed.line_start, &normalized);
    let object_hint = parsed.object_hint.unwrap_or_else(|| normalized.clone());
    Ok(ExtractedClaim {
        payload: ClaimRecorded {
            claim_id: claim_id.to_string(),
            workspace_id: workspace_id.to_string(),
            source_id: source_id.to_string(),
            source_path: doc.path.clone(),
            line_start: parsed.line_start,
            line_end: parsed.line_start,
            text: parsed.text,
            normalized_text: normalized,
            subject_hint: parsed.subject_hint.unwrap_or_else(|| "unknown".to_string()),
            predicate_hint: parsed
                .predicate_hint
                .unwrap_or_else(|| "unknown".to_string()),
            object_hint,
            confidence_ppm,
            extractor_kind: extractor_kind.to_string(),
            observed_at_ms,
        },
    })
}

/// Join a byte-collecting reader thread, discarding a panic.
fn collect_bytes(handle: thread::JoinHandle<Vec<u8>>) -> Vec<u8> {
    handle.join().unwrap_or_default()
}

/// Join the stderr-draining thread and decode its bytes lossily.
fn collect_stderr(handle: thread::JoinHandle<Vec<u8>>) -> String {
    String::from_utf8_lossy(&collect_bytes(handle))
        .trim()
        .to_string()
}

/// Attach captured stderr context to a `Cmd` error; pass other variants through.
fn append_stderr(err: ExtractError, stderr_text: &str) -> ExtractError {
    match err {
        ExtractError::Cmd(msg) => ExtractError::Cmd(format!("{msg}{}", stderr_suffix(stderr_text))),
        other => other,
    }
}

/// Build a `; stderr: ...` suffix for error messages when stderr is non-empty.
fn stderr_suffix(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!("; stderr: {stderr}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ids::SourceId;
    use assert_matches::assert_matches;

    fn source_id() -> SourceId {
        let doc = MarkdownDocument::from_bytes("t.md", b"x\n").expect("doc");
        SourceId::try_from(doc.source_id.as_str()).expect("source id")
    }

    fn run(
        cmd: &str,
        doc: &MarkdownDocument,
        timeout: Duration,
    ) -> Result<Vec<ExtractedClaim>, ExtractError> {
        extract_via_cmd_with_timeout(cmd, doc, &source_id(), "demo", 0, Path::new("."), timeout)
    }

    #[test]
    fn line_start_out_of_range_is_rejected() {
        // Document has 2 raw lines; line_start 99 is out of range.
        let doc = MarkdownDocument::from_bytes("t.md", b"alpha\nbeta\n").expect("doc");
        let cmd = r#"printf '{"line_start": 99, "text": "x"}\n'; :"#;
        let err = run(cmd, &doc, Duration::from_secs(5)).expect_err("should reject");
        assert_matches!(err, ExtractError::Cmd(msg) if msg.contains("out of range"));
    }

    #[test]
    fn claim_after_code_fence_is_accepted() {
        // Regression guard: line_start points at a content line whose raw file
        // number (7) exceeds the FILTERED doc.lines.len() (4 retained lines)
        // because the fenced code block drops lines. It must NOT be rejected.
        let body = b"# Title\n\n```\ncode\n```\n\nDeploys happen on Friday.\n";
        let doc = MarkdownDocument::from_bytes("t.md", body).expect("doc");
        // Filtered line list is shorter than the raw file.
        assert!(doc.lines.len() < 7);
        assert_eq!(max_raw_line(&doc), 7);

        let cmd = r#"printf '{"line_start": 7, "text": "Deploys happen on Friday."}\n'"#;
        let claims = run(cmd, &doc, Duration::from_secs(5)).expect("should accept");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].payload.line_start, 7);
        assert_eq!(claims[0].payload.line_end, 7);
    }

    #[test]
    fn confidence_ppm_over_max_is_rejected() {
        let doc = MarkdownDocument::from_bytes("t.md", b"alpha\nbeta\n").expect("doc");
        let cmd = r#"printf '{"line_start": 1, "text": "x", "confidence_ppm": 1000001}\n'"#;
        let err = run(cmd, &doc, Duration::from_secs(5)).expect_err("should reject");
        assert_matches!(err, ExtractError::Cmd(msg) if msg.contains("confidence_ppm"));
    }

    #[test]
    fn missing_confidence_ppm_uses_shared_default() {
        // A JSON line omitting confidence_ppm must fall back to the single shared
        // extraction default, matching the heuristic path (not an ad-hoc value).
        let doc = MarkdownDocument::from_bytes("t.md", b"alpha\nbeta\n").expect("doc");
        let cmd = r#"printf '{"line_start": 1, "text": "x"}\n'"#;
        let claims = run(cmd, &doc, Duration::from_secs(5)).expect("should accept");
        assert_eq!(claims.len(), 1);
        assert_eq!(
            claims[0].payload.confidence_ppm,
            super::super::DEFAULT_CONFIDENCE_PPM
        );
    }

    #[test]
    fn confidence_ppm_at_max_is_accepted() {
        let doc = MarkdownDocument::from_bytes("t.md", b"alpha\nbeta\n").expect("doc");
        let cmd = r#"printf '{"line_start": 1, "text": "x", "confidence_ppm": 1000000}\n'"#;
        let claims = run(cmd, &doc, Duration::from_secs(5)).expect("should accept");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].payload.confidence_ppm, 1_000_000);
    }

    #[test]
    fn invalid_json_line_surfaces_cmd_json_error() {
        // A non-JSON stdout line must fail as ExtractError::CmdJson, not be
        // silently dropped.
        let doc = MarkdownDocument::from_bytes("t.md", b"alpha\nbeta\n").expect("doc");
        let cmd = r"printf 'not-json\n'";
        let err = run(cmd, &doc, Duration::from_secs(5)).expect_err("should reject");
        assert_matches!(err, ExtractError::CmdJson(_));
    }

    #[test]
    fn spawn_in_missing_directory_surfaces_cmd_io_error() {
        // Running with a non-existent working directory makes the process spawn
        // fail; the adapter must surface ExtractError::CmdIo with the spawn
        // context, not panic.
        let doc = MarkdownDocument::from_bytes("t.md", b"alpha\n").expect("doc");
        let missing = Path::new("/nonexistent/texo/working/dir/should/not/exist");
        let err = extract_via_cmd_with_timeout(
            "true",
            &doc,
            &source_id(),
            "demo",
            0,
            missing,
            Duration::from_secs(5),
        )
        .expect_err("spawn in missing dir must fail");
        assert_matches!(err, ExtractError::CmdIo { context, .. } if context == "spawn failed");
    }

    #[test]
    fn nonzero_exit_surfaces_cmd_error() {
        // An extractor that emits valid claims but exits non-zero must still be
        // reported as a failure (Cmd error), not silently accepted.
        let doc = MarkdownDocument::from_bytes("t.md", b"alpha\nbeta\n").expect("doc");
        let cmd = r#"printf '{"line_start": 1, "text": "x"}\n'; exit 3; :"#;
        let err = run(cmd, &doc, Duration::from_secs(5)).expect_err("nonzero exit must error");
        assert_matches!(err, ExtractError::Cmd(msg) if msg.contains("exited"));
    }

    #[test]
    fn nonzero_exit_with_stderr_appends_stderr_suffix() {
        // A non-zero exit that also wrote to stderr must surface the stderr text
        // in the error message via the `; stderr: ...` suffix path.
        let doc = MarkdownDocument::from_bytes("t.md", b"alpha\nbeta\n").expect("doc");
        let cmd = r"printf 'boom-on-stderr\n' 1>&2; exit 4; :";
        let err = run(cmd, &doc, Duration::from_secs(5)).expect_err("nonzero exit must error");
        assert_matches!(
            err,
            ExtractError::Cmd(msg)
                if msg.contains("exited") && msg.contains("stderr: boom-on-stderr")
        );
    }

    #[test]
    fn stderr_suffix_empty_when_no_stderr() {
        // Direct unit check of the suffix helper: empty stderr yields no suffix,
        // non-empty yields the labelled suffix.
        assert_eq!(stderr_suffix(""), "");
        assert_eq!(stderr_suffix("oops"), "; stderr: oops");
    }

    #[test]
    fn times_out_when_child_holds_stdout_open() {
        // Child prints one line then sleeps for a long time while holding stdout
        // open. With a short timeout, extract_via_cmd must return a timeout error
        // well before the child would finish, and within a bounded wall time.
        let doc = MarkdownDocument::from_bytes("t.md", b"alpha\nbeta\n").expect("doc");
        // `sleep 120` keeps stdout open (no EOF) far past the 200ms timeout.
        // The harness appends `"$1"` (the doc path) to the command; trailing
        // `; :` lets the `:` no-op builtin swallow that argument harmlessly.
        let cmd = r#"printf '{"line_start": 1, "text": "x"}\n'; sleep 120; :"#;
        let start = Instant::now();
        let err = run(cmd, &doc, Duration::from_millis(200)).expect_err("should time out");
        let elapsed = start.elapsed();
        assert_matches!(err, ExtractError::Cmd(msg) if msg.contains("timed out"));
        // Generous bound to avoid CI flakiness, but far below the child's 120s.
        assert!(elapsed < Duration::from_secs(10));
    }
}
