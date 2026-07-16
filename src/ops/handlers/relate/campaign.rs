use super::super::common::{append_json, semantic_temporal_policy};
use super::{RelateConflictRow, RelateSupersessionRow, RELATE_PREFILTER};
use crate::claims::campaign::CampaignCard;
use crate::claims::status::ClaimStatus;
use crate::claims::workspace::WorkspaceView;
use crate::error::TexoError;
use crate::events::coordinate::entity_for_relation_campaign;
use crate::events::ids::WorkspaceId;
use crate::events::payloads::RelationCampaignCheckpointV1;
use crate::gateway::{
    resolve_role_with_environment, GatewayConfig, GatewayEnvironment, ModelRole, RoleOverrides,
    ENV_BASE_URL,
};
use crate::ops::env;
use crate::relate::settlement::CampaignPhase;
use crate::semantics::pipeline::{CandidateCursor, RelateTemporalPolicy};
use batpak::event::{EventPayload, EventSourced};
use std::collections::BTreeMap;

const CANDIDATE_POLICY_VERSION: &str = "texo.relate.candidates.v1";
const CAMPAIGN_BASIS_VERSION: &str = "texo.relate.campaign-basis.v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CampaignBasis {
    claims: BTreeMap<String, bool>,
    temporal_identity_digest_hex: String,
}

impl CampaignBasis {
    pub(super) fn from_view(view: &WorkspaceView, temporal: &RelateTemporalPolicy) -> Self {
        Self::from_entries(
            view.claims.iter().map(|claim| {
                (
                    claim.card.claim_id.clone(),
                    claim.status == ClaimStatus::Current,
                )
            }),
            temporal,
        )
    }

    fn from_entries(
        entries: impl IntoIterator<Item = (String, bool)>,
        temporal: &RelateTemporalPolicy,
    ) -> Self {
        Self {
            claims: entries.into_iter().collect(),
            temporal_identity_digest_hex: temporal.campaign_identity_digest(),
        }
    }

    pub(super) fn digest(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        hash_field(&mut hasher, CAMPAIGN_BASIS_VERSION.as_bytes());
        for (claim_id, eligible) in &self.claims {
            hash_field(&mut hasher, claim_id.as_bytes());
            hasher.update(&[u8::from(*eligible)]);
        }
        hash_field(&mut hasher, self.temporal_identity_digest_hex.as_bytes());
        hasher.finalize().to_hex().to_string()
    }

    pub(super) fn after_publication(
        &self,
        supersessions: &[RelateSupersessionRow],
        conflicts: &[RelateConflictRow],
    ) -> Self {
        let mut result = self.clone();
        for row in supersessions {
            result.claims.insert(row.old_claim_id.clone(), false);
        }
        for row in conflicts {
            result.claims.insert(row.claim_a.clone(), false);
            result.claims.insert(row.claim_b.clone(), false);
        }
        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CampaignStart {
    AlreadyComplete,
    Run(CandidateCursor),
}

pub(super) fn resolve_start(
    latest: Option<&RelationCampaignCheckpointV1>,
    basis_digest: &str,
    policy_digest: &str,
    requested: Option<CandidateCursor>,
) -> Result<CampaignStart, TexoError> {
    let matching = latest.filter(|checkpoint| {
        checkpoint.result_basis_digest_hex == basis_digest
            && checkpoint.candidate_policy_digest_hex == policy_digest
    });
    match matching.map(|checkpoint| checkpoint.phase) {
        Some(CampaignPhase::Complete) => Ok(CampaignStart::AlreadyComplete),
        Some(CampaignPhase::Partial {
            next_candidate_cursor,
        }) => match requested {
            None => Ok(CampaignStart::Run(CandidateCursor::from_offset(
                next_candidate_cursor,
            ))),
            Some(cursor) if cursor.offset() == 0 || cursor.offset() == next_candidate_cursor => Ok(
                CampaignStart::Run(CandidateCursor::from_offset(next_candidate_cursor)),
            ),
            Some(cursor) => Err(cursor_mismatch(cursor.offset(), next_candidate_cursor)),
        },
        None => match requested {
            None => Ok(CampaignStart::Run(CandidateCursor::start())),
            Some(cursor) if cursor.offset() == 0 => Ok(CampaignStart::Run(cursor)),
            Some(cursor) => Err(TexoError::OpInput {
                op: "texo.relate.run".to_string(),
                detail: format!(
                    "candidate cursor {} has no matching durable campaign; restart from cursor 0",
                    cursor.offset()
                ),
            }),
        },
    }
}

pub(super) fn latest_checkpoint(
    view: &WorkspaceView,
) -> Result<Option<RelationCampaignCheckpointV1>, TexoError> {
    env::with(|op_env| {
        env::deterministic_projection(|| {
            let entity = entity_for_relation_campaign(&view.workspace_id);
            let mut card = CampaignCard::default();
            for entry in op_env.store.by_entity(&entity) {
                if entry.global_sequence() > view.frontier {
                    break;
                }
                if entry.event_kind() != <RelationCampaignCheckpointV1 as EventPayload>::KIND {
                    continue;
                }
                let raw = op_env.store.read_raw(entry.event_id())?;
                card.apply_event(&raw.event);
            }
            Ok::<_, TexoError>(card.latest)
        })
    })?
}

pub(super) fn candidate_policy_digest(
    cluster: f32,
    prefilter: f32,
    gateway: Option<&GatewayConfig>,
) -> String {
    let environment = GatewayEnvironment {
        base_url: std::env::var(ENV_BASE_URL).ok(),
        api_key: None,
        model: std::env::var(ModelRole::Embed.model_env()).ok(),
    };
    candidate_policy_digest_with_environment(cluster, prefilter, gateway, &environment)
}

fn candidate_policy_digest_with_environment(
    cluster: f32,
    prefilter: f32,
    gateway: Option<&GatewayConfig>,
    environment: &GatewayEnvironment,
) -> String {
    let resolved = resolve_role_with_environment(
        ModelRole::Embed,
        &RoleOverrides::default(),
        gateway,
        environment,
    );
    let mut hasher = blake3::Hasher::new();
    hash_field(&mut hasher, CANDIDATE_POLICY_VERSION.as_bytes());
    hash_field(&mut hasher, &cluster.to_bits().to_be_bytes());
    hash_field(&mut hasher, &prefilter.to_bits().to_be_bytes());
    hash_field(&mut hasher, resolved.provider_id.as_bytes());
    hash_field(&mut hasher, resolved.profile.base_url.as_bytes());
    hash_field(&mut hasher, resolved.config.model.as_bytes());
    hasher.finalize().to_hex().to_string()
}

pub(super) struct CheckpointDraft {
    pub(super) workspace_id: WorkspaceId,
    pub(super) evaluated_basis_digest_hex: String,
    pub(super) result_basis_digest_hex: String,
    pub(super) candidate_policy_digest_hex: String,
    pub(super) phase: CampaignPhase,
    pub(super) observed_at_ms: u64,
}

pub(super) fn append_checkpoint(
    op: &str,
    cx: &mut syncbat::Ctx<'_>,
    draft: CheckpointDraft,
) -> Result<(), TexoError> {
    append_json(
        op,
        cx,
        <RelationCampaignCheckpointV1 as EventPayload>::KIND,
        &RelationCampaignCheckpointV1 {
            workspace_id: draft.workspace_id,
            evaluated_basis_digest_hex: draft.evaluated_basis_digest_hex,
            result_basis_digest_hex: draft.result_basis_digest_hex,
            candidate_policy_digest_hex: draft.candidate_policy_digest_hex,
            phase: draft.phase,
            observed_at_ms: draft.observed_at_ms,
        },
    )
}

struct SettlementGate {
    latest: Option<RelationCampaignCheckpointV1>,
    complete: bool,
}

fn settlement_gate(view: &WorkspaceView) -> Result<SettlementGate, TexoError> {
    let (cluster, prefilter, gateway) = env::with(|op_env| {
        let semantics = op_env.config.semantics.as_ref();
        let cluster = semantics.map_or_else(
            || crate::config::SemanticsConfig::default().cosine_threshold,
            |config| config.cosine_threshold,
        );
        let prefilter = semantics
            .and_then(|config| config.relate_prefilter)
            .unwrap_or(RELATE_PREFILTER);
        (cluster, prefilter, op_env.config.gateway.clone())
    })?;
    let temporal = semantic_temporal_policy(view)?;
    let basis_digest = CampaignBasis::from_view(view, &temporal).digest();
    let policy_digest = candidate_policy_digest(cluster, prefilter, gateway.as_ref());
    let latest = latest_checkpoint(view)?;
    let complete = latest.as_ref().is_some_and(|checkpoint| {
        checkpoint.result_basis_digest_hex == basis_digest
            && checkpoint.candidate_policy_digest_hex == policy_digest
            && checkpoint.phase == CampaignPhase::Complete
    });
    Ok(SettlementGate { latest, complete })
}

pub(in crate::ops::handlers) fn settlement_is_complete(
    view: &WorkspaceView,
) -> Result<bool, TexoError> {
    Ok(settlement_gate(view)?.complete)
}

pub(in crate::ops::handlers) fn require_complete_settlement(
    view: &WorkspaceView,
) -> Result<(), TexoError> {
    let gate = settlement_gate(view)?;
    if gate.complete {
        return Ok(());
    }
    let state = gate.latest.as_ref().map_or_else(
        || "no durable campaign checkpoint".to_string(),
        |checkpoint| match checkpoint.phase {
            CampaignPhase::Complete => "checkpoint does not match this exact frontier".to_string(),
            CampaignPhase::Partial {
                next_candidate_cursor,
            } => format!("campaign is partial at candidate cursor {next_candidate_cursor}"),
        },
    );
    Err(TexoError::Semantics {
        backend: "settlement".to_string(),
        detail: format!(
            "strict settlement refused authority-bearing output: {state}; run `texo relate` to resume"
        ),
    })
}

fn cursor_mismatch(requested: u64, expected: u64) -> TexoError {
    TexoError::OpInput {
        op: "texo.relate.run".to_string(),
        detail: format!(
            "candidate cursor {requested} does not match durable resume cursor {expected}"
        ),
    }
}

fn hash_field(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_be_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoint(phase: CampaignPhase) -> RelationCampaignCheckpointV1 {
        RelationCampaignCheckpointV1 {
            workspace_id: WorkspaceId::try_from("workspace").expect("valid workspace"),
            evaluated_basis_digest_hex: "a".repeat(64),
            result_basis_digest_hex: "a".repeat(64),
            candidate_policy_digest_hex: "b".repeat(64),
            phase,
            observed_at_ms: 1,
        }
    }

    #[test]
    fn campaign_basis_is_order_independent_and_tracks_ineligible_claims() {
        let temporal = RelateTemporalPolicy::default();
        let first = CampaignBasis::from_entries(
            [
                ("claim-b".to_string(), true),
                ("claim-a".to_string(), false),
            ],
            &temporal,
        );
        let reordered = CampaignBasis::from_entries(
            [
                ("claim-a".to_string(), false),
                ("claim-b".to_string(), true),
            ],
            &temporal,
        );
        let with_new_ineligible = CampaignBasis::from_entries(
            [
                ("claim-a".to_string(), false),
                ("claim-b".to_string(), true),
                ("claim-c".to_string(), false),
            ],
            &temporal,
        );
        assert_eq!(first.digest(), reordered.digest());
        assert_ne!(first.digest(), with_new_ineligible.digest());
    }

    #[test]
    fn publication_changes_only_affected_eligibility() {
        let temporal = RelateTemporalPolicy::default();
        let basis = CampaignBasis::from_entries(
            [
                ("a".to_string(), true),
                ("b".to_string(), true),
                ("c".to_string(), true),
            ],
            &temporal,
        );
        let result = basis.after_publication(
            &[RelateSupersessionRow {
                old_claim_id: "a".to_string(),
                new_claim_id: "b".to_string(),
                reason: String::new(),
                cache_key: String::new(),
            }],
            &[RelateConflictRow {
                conflict_id: "conflict".to_string(),
                claim_a: "b".to_string(),
                claim_b: "c".to_string(),
                reason: String::new(),
                cache_key: String::new(),
            }],
        );
        assert_eq!(
            result.claims.values().copied().collect::<Vec<_>>(),
            [false; 3]
        );
    }

    #[test]
    fn temporal_evidence_changes_campaign_basis() {
        let empty = RelateTemporalPolicy::default();
        let mut rebound = RelateTemporalPolicy::default();
        rebound.bind_claim(
            &crate::events::ids::ClaimId::try_from("claim_aaaaaaaaaaaa").expect("valid claim"),
            &crate::knowledge::SourceSnapshotId::derive("snapshot"),
        );
        let claims = [("claim_aaaaaaaaaaaa".to_string(), true)];
        let first = CampaignBasis::from_entries(claims.clone(), &empty);
        let second = CampaignBasis::from_entries(claims, &rebound);
        assert_ne!(first.digest(), second.digest());
    }

    #[test]
    fn durable_cursor_is_resumed_and_mismatches_are_rejected() {
        let partial = checkpoint(CampaignPhase::Partial {
            next_candidate_cursor: 42,
        });
        let resumed = resolve_start(Some(&partial), &"a".repeat(64), &"b".repeat(64), None)
            .expect("durable cursor resumes");
        assert_eq!(
            resumed,
            CampaignStart::Run(CandidateCursor::from_offset(42))
        );
        assert!(resolve_start(
            Some(&partial),
            &"a".repeat(64),
            &"b".repeat(64),
            Some(CandidateCursor::from_offset(7))
        )
        .is_err());
        let complete = checkpoint(CampaignPhase::Complete);
        let completed = resolve_start(Some(&complete), &"a".repeat(64), &"b".repeat(64), None)
            .expect("complete campaign resolves");
        assert_eq!(completed, CampaignStart::AlreadyComplete);
    }

    #[test]
    fn candidate_policy_digest_excludes_api_key() {
        let first = GatewayEnvironment {
            base_url: Some("https://models.example/v1".to_string()),
            api_key: Some("first-secret".to_string()),
            model: Some("embed-model".to_string()),
        };
        let second = GatewayEnvironment {
            api_key: Some("second-secret".to_string()),
            ..first.clone()
        };
        assert_eq!(
            candidate_policy_digest_with_environment(0.8, 0.6, None, &first),
            candidate_policy_digest_with_environment(0.8, 0.6, None, &second)
        );
    }
}
