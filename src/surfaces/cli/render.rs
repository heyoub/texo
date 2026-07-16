//! CLI render helpers.

use std::io::{self, Write as _};

use serde_json::Value;

use crate::error::TexoError;

/// Render one typed CLI failure with its causal chain and recovery facts.
///
/// # Errors
/// Returns an I/O error when standard error cannot be written.
pub fn cli_error(error: &TexoError) -> io::Result<()> {
    use std::error::Error as _;

    let stderr = io::stderr();
    let mut out = stderr.lock();
    writeln!(out, "error[{}]: {error}", error.code())?;
    let mut source = error.source();
    while let Some(cause) = source {
        writeln!(out, "caused by: {cause}")?;
        source = cause.source();
    }
    let facts = error.facts();
    writeln!(out, "committed: {}", facts.committed)?;
    writeln!(
        out,
        "retry: {}",
        if facts.retry_safe { "safe" } else { "unsafe" }
    )?;
    if let Some(resume) = facts.resume {
        writeln!(out, "resume: {resume}")?;
    }
    Ok(())
}

/// Print a JSON value unchanged except for pretty formatting.
///
/// # Errors
///
/// Returns [`TexoError::Json`] when the value cannot be serialized.
pub fn json(value: &Value) -> Result<(), TexoError> {
    let stdout = io::stdout();
    writeln!(stdout.lock(), "{}", serde_json::to_string_pretty(value)?)?;
    Ok(())
}

/// Print the operation catalog in one stable line per operation.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn operations(value: &Value) -> Result<(), TexoError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let operations = value
        .get("operations")
        .and_then(Value::as_array)
        .map_or(&[] as &[Value], Vec::as_slice);
    for operation in operations {
        let name = operation
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let effect = operation
            .get("effect")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let agent = operation
            .get("agent_tool")
            .and_then(Value::as_str)
            .map_or("human", |tool| tool);
        writeln!(out, "{name}\t{effect}\t{agent}")?;
    }
    Ok(())
}

/// Print the init message.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn init(root: &std::path::Path, value: &Value) -> Result<(), TexoError> {
    let workspace = value
        .get("workspace_id")
        .and_then(Value::as_str)
        .unwrap_or("demo");
    writeln!(
        io::stdout().lock(),
        "Initialized texo workspace '{}' at {}/.texo",
        workspace,
        root.display()
    )?;
    Ok(())
}

/// Print ingest summary.
///
/// # Errors
/// Returns [`TexoError::Io`] when an output stream cannot be written.
pub fn ingest(value: &Value) -> Result<(), TexoError> {
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut out = stdout.lock();
    let mut warnings = stderr.lock();
    writeln!(
        out,
        "ingested {} sources, {} claims ({})",
        value
            .get("sources_observed")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        value
            .get("claims_recorded")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        value
            .get("workspace_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
    )?;
    if value.get("empty").and_then(Value::as_bool) == Some(true) {
        writeln!(
            warnings,
            "warning: source root exists but contains no markdown sources"
        )?;
    }
    if let Some(skipped) = value.get("skipped").and_then(Value::as_array) {
        for row in skipped {
            writeln!(
                warnings,
                "warning: skipped {} ({})",
                row.get("path").and_then(Value::as_str).unwrap_or("unknown"),
                row.get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("source.io")
            )?;
        }
    }
    Ok(())
}

/// Print claims in the old four-line block format.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn claims(value: &Value) -> Result<(), TexoError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let claims = value
        .get("claims")
        .and_then(Value::as_array)
        .map_or(&[] as &[Value], Vec::as_slice);
    for claim in claims {
        let id = claim
            .get("claim_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let status = claim
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let subject = claim
            .get("subject_hint")
            .and_then(Value::as_str)
            .unwrap_or_default();
        writeln!(out, "{id} {} {subject}", status_label(status))?;
        writeln!(
            out,
            "  \"{}\"",
            claim
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )?;
        let source = claim.get("source").unwrap_or(&Value::Null);
        writeln!(
            out,
            "  source: {}:{}",
            source
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            source
                .get("line_start")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        )?;
        let receipt = claim.get("receipt").unwrap_or(&Value::Null);
        writeln!(
            out,
            "  seq: {}",
            receipt.get("sequence").and_then(Value::as_u64).unwrap_or(0)
        )?;
        writeln!(
            out,
            "  receipt: {}",
            receipt
                .get("event_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )?;
    }
    Ok(())
}

fn status_label(value: &str) -> &'static str {
    match value {
        "current" => "Current",
        "superseded" => "Superseded",
        "conflicting" => "Conflicting",
        _ => "Unknown",
    }
}

/// Print supersession summary.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn supersede(value: &Value) -> Result<(), TexoError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if value.get("already_applied").and_then(Value::as_bool) == Some(true) {
        writeln!(
            out,
            "claim {} already superseded by {} (no-op)",
            value.get("old").and_then(Value::as_str).unwrap_or_default(),
            value.get("new").and_then(Value::as_str).unwrap_or_default()
        )?;
        return Ok(());
    }
    writeln!(
        out,
        "superseded {} with {} at local seq {}",
        value.get("old").and_then(Value::as_str).unwrap_or_default(),
        value.get("new").and_then(Value::as_str).unwrap_or_default(),
        value
            .get("receipt")
            .and_then(|receipt| receipt.get("global_sequence"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    )?;
    Ok(())
}

/// Print staleness diagnostics.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn staleness(value: &Value) -> Result<(), TexoError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let diagnostics = value
        .get("diagnostics")
        .and_then(Value::as_array)
        .map_or(&[] as &[Value], Vec::as_slice);
    for diag in diagnostics {
        writeln!(
            out,
            "{}:{} warning — {}",
            diag.get("file").and_then(Value::as_str).unwrap_or_default(),
            diag.get("line_start").and_then(Value::as_u64).unwrap_or(0),
            diag.get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )?;
    }
    if diagnostics.is_empty() {
        writeln!(out, "no stale claims detected")?;
    }
    Ok(())
}

/// Print compile summary.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn compile(out: &std::path::Path, value: &Value) -> Result<(), TexoError> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let files = value
        .get("files")
        .and_then(Value::as_array)
        .map_or(&[] as &[Value], Vec::as_slice);
    for file in files {
        writeln!(
            stdout,
            "wrote {}/{}",
            out.display(),
            file.as_str().unwrap_or_default()
        )?;
    }
    Ok(())
}

/// Print conflicts.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn conflicts(value: &Value) -> Result<(), TexoError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let open = value
        .get("open")
        .and_then(Value::as_array)
        .map_or(&[] as &[Value], Vec::as_slice);
    for entry in open {
        writeln!(
            out,
            "{} {} vs {} ({})",
            entry
                .get("conflict_id")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            entry
                .get("claim_a")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            entry
                .get("claim_b")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            entry
                .get("subject_hint")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )?;
    }
    Ok(())
}

/// Print conflict commit summary.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn conflicts_committed(value: &Value) -> Result<(), TexoError> {
    writeln!(
        io::stdout().lock(),
        "committed {} conflicts",
        value.as_array().map_or(0, Vec::len)
    )?;
    Ok(())
}

/// Print verify summary.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn verify(value: &Value) -> Result<(), TexoError> {
    writeln!(
        io::stdout().lock(),
        "ok — replayed through local seq {}",
        value
            .get("replayed_through_sequence")
            .or_else(|| value.get("frontier"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    )?;
    Ok(())
}

/// Print session export markdown.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn session_markdown(markdown: &str) -> Result<(), TexoError> {
    writeln!(io::stdout().lock(), "{markdown}")?;
    Ok(())
}

/// Print serve startup line.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn serve_listening(addr: std::net::SocketAddr) -> Result<(), TexoError> {
    writeln!(io::stdout().lock(), "texo-agent listening on http://{addr}")?;
    Ok(())
}

/// Print serve bootstrap warning.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard error cannot be written.
pub fn serve_warning(message: &str) -> Result<(), TexoError> {
    writeln!(io::stderr().lock(), "{message}")?;
    Ok(())
}

/// Print a concise install or uninstall change report.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn installation(value: &Value) -> Result<(), TexoError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let verb = if value.get("workspace_id").is_some() {
        "install"
    } else {
        "uninstall"
    };
    let dry_run = value
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    writeln!(
        out,
        "texo {verb}{}",
        if dry_run { " (dry run)" } else { "" }
    )?;
    for change in value
        .get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        writeln!(
            out,
            "  {:<9} {}",
            change
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            change
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        )?;
    }
    Ok(())
}

/// Print a concise doctor report with repair guidance.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn doctor(value: &Value) -> Result<(), TexoError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    writeln!(
        out,
        "texo doctor: {}",
        value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("broken")
    )?;
    for check in value
        .get("checks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        writeln!(
            out,
            "  {:<5} {:<24} {}",
            check
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("fail"),
            check.get("id").and_then(Value::as_str).unwrap_or("unknown"),
            check
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )?;
        if let Some(fix) = check.get("fix").and_then(Value::as_str) {
            writeln!(out, "        fix: {fix}")?;
        }
    }
    Ok(())
}

/// Print a concise backup create or verification report.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn backup(value: &Value) -> Result<(), TexoError> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if let Some(verified) = value.get("verified").and_then(Value::as_bool) {
        writeln!(
            out,
            "backup {}: {}",
            value
                .get("dest")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            if verified { "verified" } else { "INVALID" }
        )?;
        for finding in value
            .get("findings")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            writeln!(
                out,
                "  {}: {}",
                finding
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("invalid"),
                finding
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            )?;
        }
    } else if value.get("chain_verified").and_then(Value::as_bool) == Some(true) {
        writeln!(
            out,
            "backup restored: {} ({} files, {} bytes; chain verified)",
            value
                .get("dest")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            value
                .get("store_file_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            value
                .get("store_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        )?;
    } else {
        writeln!(
            out,
            "backup created: {} ({} files, {} bytes)",
            value
                .get("dest")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            value
                .get("store_file_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            value
                .get("store_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        )?;
        writeln!(
            out,
            "manifest hash: {} (store this outside the backup)",
            value
                .get("manifest_hash_hex")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )?;
    }
    Ok(())
}

/// Print an extractor error with the extractor subcommand prefix.
///
/// # Errors
/// Returns an I/O error when standard error cannot be written.
pub fn extract_error(error: &dyn std::error::Error) -> io::Result<()> {
    let stderr = io::stderr();
    let mut out = stderr.lock();
    write!(out, "texo extract: {error}")?;
    let mut source = error.source();
    while let Some(cause) = source {
        write!(out, ": {cause}")?;
        source = cause.source();
    }
    writeln!(out)
}

/// Print relate summary.
///
/// # Errors
/// Returns [`TexoError::Io`] when an output stream cannot be written.
pub fn relate(value: &Value) -> Result<(), TexoError> {
    let stdout = io::stdout();
    let stderr = io::stderr();
    write_relate(&mut stdout.lock(), &mut stderr.lock(), value)?;
    Ok(())
}

fn write_relate(
    out: &mut dyn io::Write,
    warnings: &mut dyn io::Write,
    value: &Value,
) -> io::Result<()> {
    let outcome = value
        .get("outcome")
        .and_then(Value::as_str)
        .unwrap_or("partial");
    let candidate_pairs = value
        .get("candidate_pairs")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let candidate_pair_budget = value
        .get("candidate_pair_budget")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    writeln!(
        out,
        "relate {outcome}: {} claims; supersessions: {}; conflicts: {}",
        value
            .get("claims_related")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        value
            .get("supersessions")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        value
            .get("conflicts")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    )?;
    writeln!(
        out,
        "candidate pairs: {candidate_pairs} (page budget: {candidate_pair_budget})"
    )?;
    if outcome == "partial" {
        if let Some(cursor) = value.get("next_candidate_cursor").and_then(Value::as_u64) {
            writeln!(out, "resume candidate cursor: {cursor}")?;
        }
    }
    if let Some(pair) = value.get("rejudged_pair").filter(|pair| !pair.is_null()) {
        writeln!(
            out,
            "rejudged pair {} -> {}: {} -> {} (first judgment remains authoritative)",
            pair.get("older_claim")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            pair.get("newer_claim")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            pair.get("prior_relation")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            pair.get("fresh_relation")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        )?;
    }
    if let Some(rows) = value.get("warnings").and_then(Value::as_array) {
        for warning in rows.iter().filter_map(Value::as_str) {
            writeln!(warnings, "warning: {warning}")?;
        }
    }
    Ok(())
}

/// Print semantic claim↔code reconciliation summary.
///
/// # Errors
/// Returns [`TexoError::Io`] when standard output cannot be written.
pub fn reconcile(value: &Value) -> Result<(), TexoError> {
    writeln!(
        io::stdout().lock(),
        "reconciliation {}: {} accepted, {} rejected, {} unresolved, {} already linked",
        value
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("partial"),
        value
            .get("accepted")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        value.get("rejected").and_then(Value::as_u64).unwrap_or(0),
        value
            .get("unresolved")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        value
            .get("already_linked")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
