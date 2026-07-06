//! Heuristic conflict detection over v2 workspace views.

use serde::{Deserialize, Serialize};

use crate::claims::card::ClaimCard;
use crate::claims::workspace::WorkspaceView;
use crate::events::ids::{conflict_id_from_pair, ClaimId};
use crate::extract::word_match::{contains_phrase, contains_word};

const CONTRADICTION_SUBJECTS: &[&str] = &[
    "deploy-process",
    "release-process",
    "approval-process",
    "ownership",
];
const SCHEDULE_DAYS: &[&str] = &["monday", "tuesday", "wednesday", "thursday", "friday"];
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

/// Read-only conflict report entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictEntry {
    /// Conflict id.
    pub conflict_id: String,
    /// First claim.
    pub claim_a: String,
    /// Second claim.
    pub claim_b: String,
    /// Subject hint shared by both claims.
    pub subject_hint: String,
    /// Heuristic reason.
    pub reason: String,
    /// Current status.
    pub status: String,
}

/// Read-only conflict detection report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictReport {
    /// Workspace id.
    pub workspace_id: String,
    /// Detected conflicts.
    pub conflicts: Vec<ConflictEntry>,
}

/// Detect current-claim conflicts from a workspace view.
///
/// # Errors
///
/// Returns [`crate::error::TexoError::IdParse`] when a projected claim id cannot
/// be parsed back into the branded domain id used for deterministic conflict
/// ids.
pub fn detect_conflicts(view: &WorkspaceView) -> Result<ConflictReport, crate::error::TexoError> {
    let mut by_subject: std::collections::BTreeMap<String, Vec<&ClaimCard>> =
        std::collections::BTreeMap::new();
    for claim in &view.claims {
        if claim.card.phase != 1 {
            continue;
        }
        by_subject
            .entry(claim.card.subject_hint.clone().unwrap_or_default())
            .or_default()
            .push(&claim.card);
    }

    let mut conflicts = Vec::new();
    for (subject, group) in by_subject {
        for i in 0..group.len() {
            for j in (i + 1)..group.len() {
                let a = group[i];
                let b = group[j];
                if a.normalized_text == b.normalized_text
                    || !has_contradiction_signal(a, b, &subject)
                {
                    continue;
                }
                let a_id = ClaimId::try_from(a.claim_id.as_str())?;
                let b_id = ClaimId::try_from(b.claim_id.as_str())?;
                let conflict_id = conflict_id_from_pair(&a_id, &b_id).to_string();
                conflicts.push(ConflictEntry {
                    conflict_id,
                    claim_a: a.claim_id.clone(),
                    claim_b: b.claim_id.clone(),
                    subject_hint: subject.clone(),
                    reason: format!(
                        "contradictory current claims on {subject}: \"{}\" vs \"{}\"",
                        a.text, b.text
                    ),
                    status: "open".to_string(),
                });
            }
        }
    }
    conflicts.sort_by(|left, right| left.conflict_id.cmp(&right.conflict_id));
    Ok(ConflictReport {
        workspace_id: view.workspace_id.clone(),
        conflicts,
    })
}

/// Returns true when text contains a replacement keyword.
#[must_use]
pub fn has_replacement_keyword(text: &str) -> bool {
    REPLACEMENT_KEYWORDS
        .iter()
        .any(|keyword| contains_phrase(text, keyword))
}

fn has_contradiction_signal(a: &ClaimCard, b: &ClaimCard, subject: &str) -> bool {
    if has_replacement_keyword(&a.text)
        || has_replacement_keyword(&a.normalized_text)
        || has_replacement_keyword(&b.text)
        || has_replacement_keyword(&b.normalized_text)
    {
        return true;
    }
    if negation_mismatch(a, b) {
        return true;
    }
    let a_predicate = a.predicate_hint.as_deref().unwrap_or_default();
    let b_predicate = b.predicate_hint.as_deref().unwrap_or_default();
    if a_predicate != b_predicate && (a_predicate == "changed" || b_predicate == "changed") {
        return true;
    }
    let a_object = a.object_hint.as_deref().unwrap_or_default();
    let b_object = b.object_hint.as_deref().unwrap_or_default();
    if a_predicate == "owns" && b_predicate == "owns" && a_object != b_object {
        return true;
    }
    if schedule_object_clash(a_object, b_object, subject) {
        return true;
    }
    if confidence_gap(a, b) {
        return true;
    }
    CONTRADICTION_SUBJECTS.contains(&subject)
        && a_object != b_object
        && !a_object.is_empty()
        && !b_object.is_empty()
}

fn negation_mismatch(a: &ClaimCard, b: &ClaimCard) -> bool {
    let a_neg = contains_word(&a.normalized_text, "not");
    let b_neg = contains_word(&b.normalized_text, "not");
    if a_neg == b_neg {
        return false;
    }
    let strip = |s: &str| s.replace(" not ", " ").replace("not ", "");
    strip(&a.normalized_text) == strip(&b.normalized_text)
}

fn schedule_object_clash(a_object: &str, b_object: &str, subject: &str) -> bool {
    if subject != "deploy-process" && subject != "release-process" {
        return false;
    }
    let a_day = SCHEDULE_DAYS
        .iter()
        .find(|day| contains_word(a_object, day));
    let b_day = SCHEDULE_DAYS
        .iter()
        .find(|day| contains_word(b_object, day));
    matches!((a_day, b_day), (Some(left), Some(right)) if left != right)
}

fn confidence_gap(a: &ClaimCard, b: &ClaimCard) -> bool {
    const THRESHOLD: u32 = 250_000;
    a.confidence_ppm.abs_diff(b.confidence_ppm) > THRESHOLD
        && a.normalized_text != b.normalized_text
}
