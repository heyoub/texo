//! CLI render helpers.

use serde_json::Value;

use crate::error::TexoError;

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
