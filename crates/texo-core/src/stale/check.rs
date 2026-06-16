//! Staleness checking against replayed claim state.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::extract::normalize::normalize_line;
use crate::extract::word_match::{contains_phrase, contains_word};
use crate::replay::state::{ClaimState, ClaimView};
use crate::source::{collect_markdown_files, MarkdownDocument};
use crate::stale::diagnostic::{
    DiagnosticSeverity, DiagnosticSource, StaleDiagnostic, StalenessReport,
};
use crate::types::ids::{claim_id_from_parts, ClaimId, SourceId, WorkspaceId};
use crate::types::status::ClaimStatus;
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
    workspace_id: &WorkspaceId,
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
        let source_id = SourceId::try_from(doc.source_id.as_str())?;

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

            let supersession = state.superseded.get(&claim.claim_id);
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
        workspace_id: workspace_id.clone(),
        checked_path,
        replayed_through_sequence: state.replayed_through_sequence,
        diagnostics,
    })
}

/// Infer supersession edges during ingest ordering.
///
/// `new_claims` are the claims recorded in the current ingest batch. `historical_claims`
/// are the workspace's currently-active claims loaded from the journal; they participate
/// as supersession candidates but can never themselves be the superseding (winning) claim.
/// This guarantees an edge is only emitted when a new claim supersedes an older one, and
/// never purely between two pre-existing historical claims (those were resolved at their own
/// ingest time). `existing_edges` lists `(old_claim_id, new_claim_id)` pairs already recorded
/// in the journal so duplicate edges are not re-emitted.
pub fn infer_supersessions(
    new_claims: &[(ClaimId, ClaimView)],
    historical_claims: &[(ClaimId, ClaimView)],
    existing_edges: &HashSet<(ClaimId, ClaimId)>,
) -> Vec<(ClaimId, ClaimId, String)> {
    let new_ids: HashSet<ClaimId> = new_claims.iter().map(|(id, _)| id.clone()).collect();

    let mut by_subject: HashMap<String, Vec<(ClaimId, ClaimView)>> = HashMap::new();
    for (id, view) in new_claims.iter().chain(historical_claims.iter()) {
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

        // The superseding claim must come from the current batch; restrict winner
        // candidates accordingly so historical claims never supersede each other.
        let mut winners: Vec<(ClaimId, ClaimView)> = group
            .iter()
            .filter(|(id, _)| new_ids.contains(id))
            .filter(|(_, v)| {
                has_replacement_keyword(&v.text) || has_replacement_keyword(&v.normalized_text)
            })
            .cloned()
            .collect();

        if winners.is_empty() {
            // Fall back to the last new-batch claim in insertion order.
            let Some(latest_new) = group.iter().rev().find(|(id, _)| new_ids.contains(id)) else {
                continue;
            };
            winners.push(latest_new.clone());
        }

        winners.sort_by_key(|(_, v)| supersession_canonical_rank(v));
        let Some(canonical) = winners.last() else {
            continue;
        };

        for (candidate_id, candidate) in &group {
            if candidate_id == &canonical.0 {
                continue;
            }
            if candidate.normalized_text == canonical.1.normalized_text {
                continue;
            }
            if existing_edges.contains(&(candidate_id.clone(), canonical.0.clone())) {
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
    // Deterministic ordering independent of HashMap iteration order.
    edges.sort_by(|a, b| {
        a.0.as_str()
            .cmp(b.0.as_str())
            .then_with(|| a.1.as_str().cmp(b.1.as_str()))
    });
    edges
}

/// Returns true when text contains a replacement keyword used for supersession inference.
pub fn has_replacement_keyword(text: &str) -> bool {
    REPLACEMENT_KEYWORDS
        .iter()
        .any(|k| contains_phrase(text, k))
}

/// Rank candidate supersession winners: substantive replacements beat meta negations.
fn supersession_canonical_rank(view: &ClaimView) -> (u8, u64) {
    let text = &view.text;
    let tier = if contains_phrase(text, "no longer")
        && !contains_word(text, "moved")
        && !contains_word(text, "changed")
        && !contains_word(text, "decided")
    {
        0
    } else if contains_word(text, "moved")
        || contains_word(text, "changed")
        || contains_word(text, "decided")
        || contains_phrase(text, "happen on")
        || contains_word(text, "now")
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

    use crate::types::ids::SourceId;
    use crate::types::receipt::receipt_view;

    fn view(id: &str, subject: &str, text: &str, sequence: u64) -> (ClaimId, ClaimView) {
        let claim_id = ClaimId::try_from(id).expect("id");
        let v = ClaimView {
            claim_id: claim_id.clone(),
            workspace_id: "demo".to_string(),
            source_id: SourceId::try_from("src_abc123def456").expect("src"),
            source_path: "x.md".to_string(),
            line_start: u32::try_from(sequence).unwrap_or(u32::MAX),
            line_end: u32::try_from(sequence).unwrap_or(u32::MAX),
            text: text.to_string(),
            normalized_text: normalize_line(text),
            subject_hint: subject.to_string(),
            predicate_hint: "unknown".to_string(),
            object_hint: text.to_ascii_lowercase(),
            confidence_ppm: 650_000,
            extractor_kind: "test".to_string(),
            status: ClaimStatus::Current,
            receipt: receipt_view(
                sequence.into(),
                sequence,
                "ClaimRecorded",
                "workspace:demo",
                id,
            ),
            supersedes: Vec::new(),
            superseded_by: None,
        };
        (claim_id, v)
    }

    #[test]
    fn infer_supersession_keyword_winner_supersedes_older() {
        // Two new-batch claims on one subject; the one carrying a replacement
        // keyword ("moved") is the winner and supersedes the other.
        let new_claims = vec![
            view(
                "claim_aaaaaaaaaaaa",
                "deploy-process",
                "Deploys happen Friday.",
                1,
            ),
            view(
                "claim_bbbbbbbbbbbb",
                "deploy-process",
                "Deploys moved to Tuesday.",
                2,
            ),
        ];
        let edges = infer_supersessions(&new_claims, &[], &HashSet::new());
        assert_eq!(edges.len(), 1, "exactly one edge expected");
        let (old, new, reason) = &edges[0];
        assert_eq!(old.as_str(), "claim_aaaaaaaaaaaa");
        assert_eq!(new.as_str(), "claim_bbbbbbbbbbbb");
        assert!(reason.starts_with("superseded by"), "got {reason}");
    }

    #[test]
    fn infer_supersession_fallback_to_latest_new_claim() {
        // No replacement keyword present, so winners is empty and the fallback
        // picks the LAST new-batch claim in insertion order as the winner.
        let new_claims = vec![
            view(
                "claim_aaaaaaaaaaaa",
                "deploy-process",
                "Deploys on Friday.",
                1,
            ),
            view(
                "claim_bbbbbbbbbbbb",
                "deploy-process",
                "Deploys on Tuesday.",
                2,
            ),
        ];
        let edges = infer_supersessions(&new_claims, &[], &HashSet::new());
        assert_eq!(edges.len(), 1);
        let (old, new, _) = &edges[0];
        // On the fallback path the latest new-batch claim wins.
        assert_eq!(new.as_str(), "claim_bbbbbbbbbbbb");
        assert_eq!(old.as_str(), "claim_aaaaaaaaaaaa");
    }

    #[test]
    fn infer_supersession_skips_existing_edge() {
        // The candidate->winner edge already exists in the journal, so it must
        // NOT be re-emitted.
        let new_claims = vec![
            view(
                "claim_aaaaaaaaaaaa",
                "deploy-process",
                "Deploys happen Friday.",
                1,
            ),
            view(
                "claim_bbbbbbbbbbbb",
                "deploy-process",
                "Deploys moved to Tuesday.",
                2,
            ),
        ];
        let mut existing = HashSet::new();
        existing.insert((
            ClaimId::try_from("claim_aaaaaaaaaaaa").expect("id"),
            ClaimId::try_from("claim_bbbbbbbbbbbb").expect("id"),
        ));
        let edges = infer_supersessions(&new_claims, &[], &existing);
        assert!(edges.is_empty(), "existing edge must not be re-emitted");
    }

    #[test]
    fn infer_supersession_no_historical_only_winner_yields_no_edge() {
        // A subject group of size 1 (single new claim) cannot supersede anything.
        let new_claims = vec![view(
            "claim_aaaaaaaaaaaa",
            "deploy-process",
            "Deploys moved to Tuesday.",
            1,
        )];
        let edges = infer_supersessions(&new_claims, &[], &HashSet::new());
        assert!(edges.is_empty(), "single-claim subject has no edges");
    }

    #[test]
    fn infer_supersession_skips_historical_only_group() {
        // Group has >=2 members but all are historical (no new claim): there is
        // no winner candidate from the batch, so no edge is emitted.
        let historical = vec![
            view("claim_aaaaaaaaaaaa", "deploy-process", "Deploys Friday.", 1),
            view(
                "claim_bbbbbbbbbbbb",
                "deploy-process",
                "Deploys moved Tuesday.",
                2,
            ),
        ];
        let edges = infer_supersessions(&[], &historical, &HashSet::new());
        // A purely historical group has no batch winner, so no edge is emitted.
        assert!(edges.is_empty());
    }

    #[test]
    fn infer_supersession_equal_normalized_text_not_superseded() {
        // The winner and a candidate share identical normalized text -> they are
        // duplicates, so no edge between them. A distinct third claim still gets
        // superseded.
        let new_claims = vec![
            view(
                "claim_aaaaaaaaaaaa",
                "deploy-process",
                "Deploys moved to Tuesday.",
                1,
            ),
            view(
                "claim_bbbbbbbbbbbb",
                "deploy-process",
                "Deploys moved to Tuesday.",
                2,
            ),
        ];
        let edges = infer_supersessions(&new_claims, &[], &HashSet::new());
        // Identical normalized text means duplicates, so no edge between them.
        assert!(edges.is_empty());
    }

    #[test]
    fn canonical_rank_no_longer_is_lowest_tier() {
        // "no longer" with no substantive keyword ranks tier 0 (meta negation),
        // below a plain claim (tier 1) and a substantive "changed" (tier 2).
        let meta = view(
            "claim_aaaaaaaaaaaa",
            "s",
            "Deploys are no longer on Friday.",
            1,
        )
        .1;
        let plain = view("claim_bbbbbbbbbbbb", "s", "Deploys are on Friday.", 2).1;
        let substantive = view("claim_cccccccccccc", "s", "Deploys changed to Tuesday.", 3).1;
        assert!(supersession_canonical_rank(&meta).0 < supersession_canonical_rank(&plain).0);
        assert!(
            supersession_canonical_rank(&plain).0 < supersession_canonical_rank(&substantive).0
        );
    }

    #[test]
    fn replacement_keyword_detected() {
        assert!(has_replacement_keyword("deploys moved to Tuesday"));
    }

    #[test]
    fn keyword_now_not_matched_inside_known() {
        // "now" must not match as a substring of "known".
        assert!(!has_replacement_keyword("this is a known issue"));
    }

    #[test]
    fn keyword_now_matched_as_whole_word() {
        assert!(has_replacement_keyword("deploys now happen on Tuesday"));
    }
}
