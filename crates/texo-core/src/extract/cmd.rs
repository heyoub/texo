//! External command extractor adapter.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::events::payloads::ClaimRecorded;
use crate::source::markdown::MarkdownDocument;
use crate::types::ids::{claim_id_from_parts, SourceId};

use super::{ExtractError, ExtractedClaim};

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
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("{cmd} \"$1\""))
        .arg("texo-extract")
        .arg(&doc.path)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ExtractError::Cmd(format!("spawn failed: {e}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ExtractError::Cmd("missing stdout".to_string()))?;
    let reader = BufReader::new(stdout);

    let basename = Path::new(cmd)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("cmd");
    let extractor_kind = format!("cmd:{basename}");

    let mut claims = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|e| ExtractError::Cmd(format!("read stdout: {e}")))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: CmdClaimLine = serde_json::from_str(trimmed)
            .map_err(|e| ExtractError::Cmd(format!("invalid json line: {e}")))?;
        let normalized = parsed
            .normalized_text
            .clone()
            .unwrap_or_else(|| super::normalize::normalize_line(&parsed.text));
        let claim_id = claim_id_from_parts(source_id, parsed.line_start, &normalized);
        let object_hint = parsed
            .object_hint
            .clone()
            .unwrap_or_else(|| normalized.clone());
        claims.push(ExtractedClaim {
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
                confidence_ppm: parsed.confidence_ppm.unwrap_or(700_000),
                extractor_kind: extractor_kind.clone(),
                observed_at_ms,
            },
        });
    }

    let status = child
        .wait()
        .map_err(|e| ExtractError::Cmd(format!("wait failed: {e}")))?;
    if !status.success() {
        return Err(ExtractError::Cmd(format!("extractor exited with {status}")));
    }

    Ok(claims)
}
