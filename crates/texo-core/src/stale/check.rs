//! Staleness checking against replayed claim state.

use std::collections::HashMap;
use std::path::Path;

use crate::extract::normalize::normalize_line;
use crate::replay::state::{ClaimState, ClaimView};
use crate::source::{collect_markdown_files, MarkdownDocument};
use crate::stale::diagnostic::{
    DiagnosticSeverity, DiagnosticSource, StaleDiagnostic, StalenessReport,
};
use crate::types::ids::{claim_id_from_parts, ClaimId, SourceId};
use crate::types::status::ClaimStatus;
use crate::types::IdParseError;
use crate::TexoError;

const REPLACEMENT_KEYWORDS: &[&str] = &[
    "moved",
    "changed",
    "now",
    "no longer",
    "replaced",
    "instead",
    "new process",
    "as of",
    "decided",
];

/// Check markdown paths for stale claims relative to replayed state.
pub fn check_staleness(
    state: &ClaimState,
    workspace_id: &str,
    input: &Path,
    root: &Path,
) -> Result<StalenessReport, TexoError> {
    let checked_path = input
        .strip_prefix(root)
        .unwrap_or(input)
        .to_string_lossy()
        .to_string();

    let files = collect_markdown_files(input)?;
    let mut diagnostics = Vec::new();

    for path in files {
        let doc = MarkdownDocument::from_path(&path, root)?;
        let source_id = SourceId::try_from(doc.source_id.as_str())
            .map_err(|e: IdParseError| TexoError::domain(e.to_string()))?;

        for line in &doc.lines {
            let normalized = normalize_line(&line.text);
            if normalized.is_empty() {
                continue;
            }
            let claim_id = claim_id_from_parts(&source_id, line.number, &normalized);
            let Some(claim) = state.claim(&claim_id) else {
                continue;
            };

            if claim.status != ClaimStatus::Superseded {
                continue;
            }

            let Some(superseded_by) = claim.superseded_by.clone() else {
                continue;
            };

            let superseder = state.claim(&superseded_by);
            let (source, receipt) = if let Some(s) = superseder {
                (
                    Some(DiagnosticSource {
                        path: s.source_path.clone(),
                        line_start: s.line_start,
                    }),
                    Some(s.receipt.clone()),
                )
            } else {
                (None, None)
            };

            let supersession = state.superseded.get(claim.claim_id.as_str());
            let receipt = supersession.map(|s| s.receipt.clone()).or(receipt);

            let message = format!(
                "Claim appears stale: superseded by {superseded_by} at {}.",
                receipt.as_ref().map_or_else(
                    || "unknown seq".to_string(),
                    |r| { format!("local seq {}", r.sequence.get()) }
                )
            );

            diagnostics.push(StaleDiagnostic {
                file: doc.path.clone(),
                line_start: line.number,
                line_end: line.number,
                severity: DiagnosticSeverity::Warning,
                message,
                claim_id: claim.claim_id.clone(),
                superseded_by: Some(superseded_by),
                source,
                receipt,
            });
        }
    }

    Ok(StalenessReport {
        workspace_id: workspace_id.to_string(),
        checked_path,
        replayed_through_sequence: state.replayed_through_sequence,
        diagnostics,
    })
}

/// Infer supersession edges during ingest ordering.
pub fn infer_supersessions(claims: &[(ClaimId, ClaimView)]) -> Vec<(ClaimId, ClaimId, String)> {
    let mut by_subject: HashMap<String, Vec<(ClaimId, ClaimView)>> = HashMap::new();
    for (id, view) in claims {
        by_subject
            .entry(view.subject_hint.clone())
            .or_default()
            .push((id.clone(), view.clone()));
    }

    let mut edges = Vec::new();
    for (_subject, group) in by_subject {
        if group.len() < 2 {
            continue;
        }

        let mut winners: Vec<(ClaimId, ClaimView)> = group
            .iter()
            .filter(|(_, v)| {
                has_replacement_keyword(&v.text) || has_replacement_keyword(&v.normalized_text)
            })
            .cloned()
            .collect();

        if winners.is_empty() {
            winners.push(group.last().expect("len >= 2").clone());
        }

        winners.sort_by_key(|(_, v)| supersession_canonical_rank(v));
        let canonical = winners.last().expect("non-empty winners");

        for (candidate_id, candidate) in &group {
            if candidate_id == &canonical.0 {
                continue;
            }
            if candidate.normalized_text == canonical.1.normalized_text {
                continue;
            }
            edges.push((
                candidate_id.clone(),
                canonical.0.clone(),
                format!(
                    "superseded by {}:{}",
                    canonical.1.source_path, canonical.1.line_start
                ),
            ));
        }
    }
    edges
}

/// Returns true when text contains a replacement keyword used for supersession inference.
pub fn has_replacement_keyword(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    REPLACEMENT_KEYWORDS.iter().any(|k| lower.contains(k))
}

/// Rank candidate supersession winners: substantive replacements beat meta negations.
fn supersession_canonical_rank(view: &ClaimView) -> (u8, u64) {
    let lower = view.text.to_ascii_lowercase();
    let tier = if lower.contains("no longer")
        && !lower.contains("moved")
        && !lower.contains("changed")
        && !lower.contains("decided")
    {
        0
    } else if lower.contains("moved")
        || lower.contains("changed")
        || lower.contains("decided")
        || lower.contains("happen on")
        || lower.contains("now ")
    {
        2
    } else {
        1
    };
    (tier, view.receipt.sequence.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_keyword_detected() {
        assert!(has_replacement_keyword("deploys moved to Tuesday"));
    }
}
