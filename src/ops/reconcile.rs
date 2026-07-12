//! Durable append boundary for policy-accepted claim↔code evidence.

use batpak::event::EventPayload;

use crate::error::TexoError;
use crate::events::ids::WorkspaceId;
use crate::events::payloads::{
    ClaimEvidenceLinkedV1, EvidenceOccurrenceRecordedV1, EvidenceReconciliationAcceptedV1,
};
use crate::knowledge::{EvidenceLinkMethod, EvidenceStance};
use crate::reconcile::{
    accept_proposal, CachedCandidateProposal, ReconcileAcceptedRow, ReconcileCandidate,
    POLICY_VERSION,
};

use super::handlers::append_json;

/// Apply deterministic policy and append each accepted evidence chain.
pub(crate) fn append_proposals(
    cx: &mut syncbat::Ctx<'_>,
    workspace_id: &WorkspaceId,
    observed_at_ms: u64,
    min_score_ppm: u32,
    judge_fingerprint: &str,
    proposals: Vec<CachedCandidateProposal>,
) -> Result<(Vec<ReconcileAcceptedRow>, usize), TexoError> {
    let mut accepted = Vec::new();
    let mut rejected = 0;
    for proposed in proposals {
        let Some((stance, score_ppm)) = accept_proposal(proposed.proposal.verdict, min_score_ppm)
        else {
            rejected += 1;
            continue;
        };
        let candidate = proposed.proposal.candidate;
        candidate
            .occurrence
            .validate()
            .map_err(|error| TexoError::OpInput {
                op: "texo.knowledge.reconcile".to_string(),
                detail: error.to_string(),
            })?;
        append_events(
            cx,
            workspace_id,
            observed_at_ms,
            &AcceptedEvidence {
                judge_fingerprint,
                candidate: &candidate,
                stance,
                score_ppm,
                cache_key_hex: &proposed.cache_key_hex,
            },
        )?;
        accepted.push(ReconcileAcceptedRow {
            claim_id: candidate.claim_id.to_string(),
            occurrence_id: candidate.occurrence.occurrence_id.to_string(),
            stance,
            score_ppm,
            code_ref: format!(
                "{}:{}",
                candidate.occurrence.path, candidate.occurrence.line_range.start
            ),
            cache_key_hex: proposed.cache_key_hex,
        });
    }
    Ok((accepted, rejected))
}

struct AcceptedEvidence<'a> {
    judge_fingerprint: &'a str,
    candidate: &'a ReconcileCandidate,
    stance: EvidenceStance,
    score_ppm: u32,
    cache_key_hex: &'a str,
}

fn append_events(
    cx: &mut syncbat::Ctx<'_>,
    workspace_id: &WorkspaceId,
    observed_at_ms: u64,
    accepted: &AcceptedEvidence<'_>,
) -> Result<(), TexoError> {
    append_json(
        "texo.knowledge.reconcile",
        cx,
        <EvidenceOccurrenceRecordedV1 as EventPayload>::KIND,
        &EvidenceOccurrenceRecordedV1 {
            workspace_id: workspace_id.clone(),
            occurrence: accepted.candidate.occurrence.clone(),
            observed_at_ms,
        },
    )?;
    append_json(
        "texo.knowledge.reconcile",
        cx,
        <EvidenceReconciliationAcceptedV1 as EventPayload>::KIND,
        &EvidenceReconciliationAcceptedV1 {
            workspace_id: workspace_id.clone(),
            claim_id: accepted.candidate.claim_id.clone(),
            occurrence_id: accepted.candidate.occurrence.occurrence_id.clone(),
            stance: accepted.stance,
            score_ppm: accepted.score_ppm,
            judge_fingerprint: accepted.judge_fingerprint.to_string(),
            cache_key_hex: accepted.cache_key_hex.to_string(),
            policy_version: POLICY_VERSION.to_string(),
            observed_at_ms,
        },
    )?;
    append_json(
        "texo.knowledge.reconcile",
        cx,
        <ClaimEvidenceLinkedV1 as EventPayload>::KIND,
        &ClaimEvidenceLinkedV1 {
            workspace_id: workspace_id.clone(),
            claim_id: accepted.candidate.claim_id.clone(),
            occurrence_id: accepted.candidate.occurrence.occurrence_id.clone(),
            stance: accepted.stance,
            method: EvidenceLinkMethod::SemanticPolicy,
            observed_at_ms,
        },
    )
}
