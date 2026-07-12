//! CLI render helpers.

use serde_json::Value;

use crate::error::TexoError;

/// Render one typed CLI failure with its causal chain and recovery facts.
#[expect(clippy::print_stderr, reason = "CLI error contract")]
pub fn cli_error(error: &TexoError) {
    use std::error::Error as _;

    eprintln!("error[{}]: {error}", error.code());
    let mut source = error.source();
    while let Some(cause) = source {
        eprintln!("caused by: {cause}");
        source = cause.source();
    }
    let facts = error.facts();
    eprintln!("committed: {}", facts.committed);
    eprintln!(
        "retry: {}",
        if facts.retry_safe { "safe" } else { "unsafe" }
    );
    if let Some(resume) = facts.resume {
        eprintln!("resume: {resume}");
    }
}

/// Print a JSON value unchanged except for pretty formatting.
///
/// # Errors
///
/// Returns [`TexoError::Json`] when the value cannot be serialized.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn json(value: &Value) -> Result<(), TexoError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Print the operation catalog in one stable line per operation.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn operations(value: &Value) {
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
        println!("{name}\t{effect}\t{agent}");
    }
}

/// Print the init message.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn init(root: &std::path::Path, value: &Value) {
    let workspace = value
        .get("workspace_id")
        .and_then(Value::as_str)
        .unwrap_or("demo");
    println!(
        "Initialized texo workspace '{}' at {}/.texo",
        workspace,
        root.display()
    );
}

/// Print ingest summary.
#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI output and warning contract"
)]
pub fn ingest(value: &Value) {
    println!(
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
    );
    if value.get("empty").and_then(Value::as_bool) == Some(true) {
        eprintln!("warning: source root exists but contains no markdown sources");
    }
    if let Some(skipped) = value.get("skipped").and_then(Value::as_array) {
        for row in skipped {
            eprintln!(
                "warning: skipped {} ({})",
                row.get("path").and_then(Value::as_str).unwrap_or("unknown"),
                row.get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("source.io")
            );
        }
    }
}

/// Print claims in the old four-line block format.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn claims(value: &Value) {
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
        println!("{id} {} {subject}", status_label(status));
        println!(
            "  \"{}\"",
            claim
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
        );
        let source = claim.get("source").unwrap_or(&Value::Null);
        println!(
            "  source: {}:{}",
            source
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            source
                .get("line_start")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        );
        let receipt = claim.get("receipt").unwrap_or(&Value::Null);
        println!(
            "  seq: {}",
            receipt.get("sequence").and_then(Value::as_u64).unwrap_or(0)
        );
        println!(
            "  receipt: {}",
            receipt
                .get("event_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
        );
    }
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
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn supersede(value: &Value) {
    if value.get("already_applied").and_then(Value::as_bool) == Some(true) {
        println!(
            "claim {} already superseded by {} (no-op)",
            value.get("old").and_then(Value::as_str).unwrap_or_default(),
            value.get("new").and_then(Value::as_str).unwrap_or_default()
        );
        return;
    }
    println!(
        "superseded {} with {} at local seq {}",
        value.get("old").and_then(Value::as_str).unwrap_or_default(),
        value.get("new").and_then(Value::as_str).unwrap_or_default(),
        value
            .get("receipt")
            .and_then(|receipt| receipt.get("global_sequence"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    );
}

/// Print staleness diagnostics.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn staleness(value: &Value) {
    let diagnostics = value
        .get("diagnostics")
        .and_then(Value::as_array)
        .map_or(&[] as &[Value], Vec::as_slice);
    for diag in diagnostics {
        println!(
            "{}:{} warning — {}",
            diag.get("file").and_then(Value::as_str).unwrap_or_default(),
            diag.get("line_start").and_then(Value::as_u64).unwrap_or(0),
            diag.get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
        );
    }
    if diagnostics.is_empty() {
        println!("no stale claims detected");
    }
}

/// Print compile summary.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn compile(out: &std::path::Path, value: &Value) {
    let files = value
        .get("files")
        .and_then(Value::as_array)
        .map_or(&[] as &[Value], Vec::as_slice);
    for file in files {
        println!(
            "wrote {}/{}",
            out.display(),
            file.as_str().unwrap_or_default()
        );
    }
}

/// Print conflicts.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn conflicts(value: &Value) {
    let open = value
        .get("open")
        .and_then(Value::as_array)
        .map_or(&[] as &[Value], Vec::as_slice);
    for entry in open {
        println!(
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
        );
    }
}

/// Print conflict commit summary.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn conflicts_committed(value: &Value) {
    println!(
        "committed {} conflicts",
        value.as_array().map_or(0, Vec::len)
    );
}

/// Print verify summary.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn verify(value: &Value) {
    println!(
        "ok — replayed through local seq {}",
        value
            .get("replayed_through_sequence")
            .or_else(|| value.get("frontier"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    );
}

/// Print session export markdown.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn session_markdown(markdown: &str) {
    println!("{markdown}");
}

/// Print serve startup line.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn serve_listening(addr: std::net::SocketAddr) {
    println!("texo-agent listening on http://{addr}");
}

/// Print serve bootstrap warning.
#[expect(clippy::print_stderr, reason = "CLI output contract")]
pub fn serve_warning(message: &str) {
    eprintln!("{message}");
}

/// Print a concise install or uninstall change report.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn installation(value: &Value) {
    let verb = if value.get("workspace_id").is_some() {
        "install"
    } else {
        "uninstall"
    };
    let dry_run = value
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    println!("texo {verb}{}", if dry_run { " (dry run)" } else { "" });
    for change in value
        .get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        println!(
            "  {:<9} {}",
            change
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            change
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        );
    }
}

/// Print a concise doctor report with repair guidance.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn doctor(value: &Value) {
    println!(
        "texo doctor: {}",
        value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("broken")
    );
    for check in value
        .get("checks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        println!(
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
        );
        if let Some(fix) = check.get("fix").and_then(Value::as_str) {
            println!("        fix: {fix}");
        }
    }
}

/// Print a concise backup create or verification report.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn backup(value: &Value) {
    if let Some(verified) = value.get("verified").and_then(Value::as_bool) {
        println!(
            "backup {}: {}",
            value
                .get("dest")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            if verified { "verified" } else { "INVALID" }
        );
        for finding in value
            .get("findings")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            println!(
                "  {}: {}",
                finding
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("invalid"),
                finding
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            );
        }
    } else {
        println!(
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
        );
        println!(
            "manifest hash: {} (store this outside the backup)",
            value
                .get("manifest_hash_hex")
                .and_then(Value::as_str)
                .unwrap_or_default()
        );
    }
}

/// Print an extractor error with the extractor subcommand prefix.
#[expect(clippy::print_stderr, reason = "CLI output contract")]
pub fn extract_error(error: &dyn std::error::Error) {
    eprint!("texo extract: {error}");
    let mut source = error.source();
    while let Some(cause) = source {
        eprint!(": {cause}");
        source = cause.source();
    }
    eprintln!();
}

/// Print relate summary.
#[expect(clippy::print_stdout, reason = "CLI output contract")]
pub fn relate(value: &Value) {
    println!(
        "related {} claims: {} supersessions, {} conflicts",
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
    );
}
