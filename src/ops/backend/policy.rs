//! Typed append policies for Texo domain events.

use batpak::event::{EventKind, EventPayload};
use batpak::id::{CausationId, CorrelationId, EntityIdType, IdempotencyKey};
use batpak::store::{AppendOptions, AppendPositionHint, AppendReceipt};

use crate::error::TexoError;
use crate::events::coordinate::{
    coordinate_for_claim, coordinate_for_code_index, coordinate_for_conflict,
    coordinate_for_evidence, coordinate_for_onboarding_projection,
    coordinate_for_relation_campaign, coordinate_for_relation_pair, coordinate_for_session,
    coordinate_for_source, coordinate_for_source_relation, coordinate_for_source_snapshot,
    coordinate_for_workspace_meta, entity_for_evidence, entity_for_relation_campaign,
    entity_for_session, entity_for_source_snapshot, session_lane,
};
use crate::events::ids::relation_pair_id;
use crate::events::machines::{
    ignore_conflict, open_conflict, record_claim, resolve_conflict, supersede_claim,
};
use crate::events::payloads::{
    ClaimEvidenceLinkedV1, ClaimRecordedV2, ClaimSupersededV2, CodeIndexRecordedV1,
    ConflictOpenedV2, ConflictResolvedV2, EvidenceOccurrenceRecordedV1,
    EvidenceReconciliationAcceptedV1, OnboardingCompiledV2, RelationCampaignCheckpointV1,
    RelationDeferredV1, RelationJudgedV1, SessionTurnV1, SourceObservedV2,
    SourceSnapshotRecordedV1, SourceSnapshotRelationV1, WorkspaceInitializedV2,
};
use crate::ops::env::OpEnv;
use crate::relate::settlement::CampaignPhase;

type AppendFn = fn(&OpEnv, &[u8]) -> Result<AppendReceipt, TexoError>;

struct AppendPolicy {
    kind: EventKind,
    append: AppendFn,
}

const APPEND_POLICIES: &[AppendPolicy] = &[
    policy::<ClaimRecordedV2>(append_claim_recorded),
    policy::<ClaimSupersededV2>(append_claim_superseded),
    policy::<ConflictOpenedV2>(append_conflict_opened),
    policy::<ConflictResolvedV2>(append_conflict_resolved),
    policy::<SourceObservedV2>(append_source_observed),
    policy::<OnboardingCompiledV2>(append_onboarding_compiled),
    policy::<WorkspaceInitializedV2>(append_workspace_initialized),
    policy::<RelationJudgedV1>(append_relation_judged),
    policy::<RelationDeferredV1>(append_relation_deferred),
    policy::<SourceSnapshotRecordedV1>(append_source_snapshot_recorded),
    policy::<EvidenceOccurrenceRecordedV1>(append_evidence_occurrence_recorded),
    policy::<EvidenceReconciliationAcceptedV1>(append_evidence_reconciliation_accepted),
    policy::<ClaimEvidenceLinkedV1>(append_claim_evidence_linked),
    policy::<CodeIndexRecordedV1>(append_code_index_recorded),
    policy::<SourceSnapshotRelationV1>(append_source_snapshot_relation),
    policy::<SessionTurnV1>(append_session_turn),
    policy::<RelationCampaignCheckpointV1>(append_relation_campaign_checkpoint),
];

const fn policy<T: EventPayload>(append: AppendFn) -> AppendPolicy {
    AppendPolicy {
        kind: T::KIND,
        append,
    }
}

pub(super) fn append(
    op_env: &OpEnv,
    kind: EventKind,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let selected = APPEND_POLICIES.iter().find(|policy| policy.kind == kind);
    selected.map_or_else(
        || {
            Err(TexoError::OpRuntime {
                op: "texo.effect.append".to_string(),
                detail: format!(
                    "event kind evt.{:04x} is outside texo domain",
                    kind.as_raw_u16()
                ),
                denied: false,
            })
        },
        |policy| (policy.append)(op_env, payload_bytes),
    )
}

fn append_claim_recorded(op_env: &OpEnv, payload_bytes: &[u8]) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<ClaimRecordedV2>(payload_bytes)?;
    let coordinate = coordinate_for_claim(&payload.workspace_id, &payload.claim_id)?;
    let key = IdempotencyKey::for_operation(
        "texo.claim.recorded.v2",
        &[&payload.workspace_id, &payload.claim_id],
    );
    let payload = record_claim(payload).into_payload();
    append_with_key(op_env, &coordinate, &payload, key)
}

fn append_claim_superseded(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<ClaimSupersededV2>(payload_bytes)?;
    let coordinate = coordinate_for_claim(&payload.workspace_id, &payload.old_claim_id)?;
    let key = IdempotencyKey::for_operation(
        "texo.claim.supersede.v2",
        &[
            &payload.workspace_id,
            &payload.old_claim_id,
            &payload.new_claim_id,
        ],
    );
    let payload = supersede_claim(payload).into_payload();
    append_with_key(op_env, &coordinate, &payload, key)
}

fn append_conflict_opened(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<ConflictOpenedV2>(payload_bytes)?;
    let coordinate = coordinate_for_conflict(&payload.workspace_id, &payload.conflict_id)?;
    let key = IdempotencyKey::for_operation(
        "texo.conflict.open.v2",
        &[&payload.workspace_id, &payload.conflict_id],
    );
    let payload = open_conflict(payload).into_payload();
    append_with_key(op_env, &coordinate, &payload, key)
}

fn append_conflict_resolved(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<ConflictResolvedV2>(payload_bytes)?;
    let coordinate = coordinate_for_conflict(&payload.workspace_id, &payload.conflict_id)?;
    let key = IdempotencyKey::for_operation(
        "texo.conflict.resolve.v2",
        &[&payload.workspace_id, &payload.conflict_id],
    );
    match payload.resolution.as_str() {
        "resolved" => {
            let payload = resolve_conflict(payload).into_payload();
            append_with_key(op_env, &coordinate, &payload, key)
        }
        "ignored" => {
            let payload = ignore_conflict(payload).into_payload();
            append_with_key(op_env, &coordinate, &payload, key)
        }
        other => Err(TexoError::StatusParse {
            value: other.to_string(),
        }),
    }
}

fn append_source_observed(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<SourceObservedV2>(payload_bytes)?;
    let coordinate = coordinate_for_source(&payload.workspace_id, &payload.source_id)?;
    let key = IdempotencyKey::for_operation(
        "texo.source.observed.v2",
        &[&payload.workspace_id, &payload.body_hash_hex],
    );
    append_with_key(op_env, &coordinate, &payload, key)
}

fn append_onboarding_compiled(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<OnboardingCompiledV2>(payload_bytes)?;
    let coordinate = coordinate_for_onboarding_projection(&payload.workspace_id)?;
    op_env
        .store
        .append_typed(&coordinate, &payload)
        .map_err(Into::into)
}

fn append_workspace_initialized(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<WorkspaceInitializedV2>(payload_bytes)?;
    let coordinate = coordinate_for_workspace_meta(&payload.workspace_id)?;
    let key = IdempotencyKey::for_operation(
        "texo.workspace.initialized.v2",
        &[&payload.workspace_id, &payload.config_digest_hex],
    );
    append_with_key(op_env, &coordinate, &payload, key)
}

fn append_relation_judged(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<RelationJudgedV1>(payload_bytes)?;
    let pair_id = relation_pair_id(
        &payload.workspace_id,
        &payload.older_claim,
        &payload.newer_claim,
    );
    let coordinate = coordinate_for_relation_pair(payload.workspace_id.as_str(), pair_id.as_str())?;
    let payload_identity = canonical_payload_identity(&payload)?;
    let key = IdempotencyKey::for_operation(
        "texo.relation.judged.v1.payload",
        &[
            payload.workspace_id.as_str(),
            payload.older_claim.as_str(),
            payload.newer_claim.as_str(),
            payload_identity.as_str(),
        ],
    );
    append_with_key(op_env, &coordinate, &payload, key)
}

fn append_relation_deferred(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<RelationDeferredV1>(payload_bytes)?;
    let pair_id = relation_pair_id(
        &payload.workspace_id,
        &payload.older_claim,
        &payload.newer_claim,
    );
    let coordinate = coordinate_for_relation_pair(payload.workspace_id.as_str(), pair_id.as_str())?;
    op_env
        .store
        .append_typed(&coordinate, &payload)
        .map_err(Into::into)
}

fn append_source_snapshot_recorded(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<SourceSnapshotRecordedV1>(payload_bytes)?;
    let coordinate = coordinate_for_source_snapshot(
        payload.workspace_id.as_str(),
        payload.snapshot_id.as_str(),
    )?;
    let key = IdempotencyKey::for_operation(
        "texo.source.snapshot.recorded.v1",
        &[payload.workspace_id.as_str(), payload.snapshot_id.as_str()],
    );
    append_with_key(op_env, &coordinate, &payload, key)
}

fn append_evidence_occurrence_recorded(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<EvidenceOccurrenceRecordedV1>(payload_bytes)?;
    payload
        .occurrence
        .validate()
        .map_err(|error| TexoError::OpInput {
            op: "texo.effect.append".to_string(),
            detail: error.to_string(),
        })?;
    let coordinate = coordinate_for_evidence(
        payload.workspace_id.as_str(),
        payload.occurrence.occurrence_id.as_str(),
    )?;
    let key = IdempotencyKey::for_operation(
        "texo.evidence.occurrence.recorded.v1",
        &[
            payload.workspace_id.as_str(),
            payload.occurrence.occurrence_id.as_str(),
        ],
    );
    let options = code_index_append_options(op_env, key, payload.occurrence.snapshot_id.as_str());
    op_env
        .store
        .append_typed_with_options(&coordinate, &payload, options)
        .map_err(Into::into)
}

fn append_evidence_reconciliation_accepted(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<EvidenceReconciliationAcceptedV1>(payload_bytes)?;
    let coordinate =
        coordinate_for_claim(payload.workspace_id.as_str(), payload.claim_id.as_str())?;
    let key = IdempotencyKey::for_operation(
        "texo.evidence.reconciliation.accepted.v1",
        &[
            payload.workspace_id.as_str(),
            payload.claim_id.as_str(),
            payload.occurrence_id.as_str(),
            &payload.judge_fingerprint,
            &payload.policy_version,
        ],
    );
    let options = evidence_chain_options(
        op_env,
        key,
        payload.occurrence_id.as_str(),
        <EvidenceOccurrenceRecordedV1 as EventPayload>::KIND,
    )?;
    op_env
        .store
        .append_typed_with_options(&coordinate, &payload, options)
        .map_err(Into::into)
}

fn append_claim_evidence_linked(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<ClaimEvidenceLinkedV1>(payload_bytes)?;
    let coordinate =
        coordinate_for_claim(payload.workspace_id.as_str(), payload.claim_id.as_str())?;
    let key = IdempotencyKey::for_operation(
        "texo.claim.evidence.linked.v1",
        &[
            payload.workspace_id.as_str(),
            payload.claim_id.as_str(),
            payload.occurrence_id.as_str(),
        ],
    );
    let options = evidence_chain_options(
        op_env,
        key,
        payload.occurrence_id.as_str(),
        <EvidenceReconciliationAcceptedV1 as EventPayload>::KIND,
    )?;
    op_env
        .store
        .append_typed_with_options(&coordinate, &payload, options)
        .map_err(Into::into)
}

fn append_code_index_recorded(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<CodeIndexRecordedV1>(payload_bytes)?;
    let coordinate =
        coordinate_for_code_index(payload.workspace_id.as_str(), payload.index_id.as_str())?;
    let key = IdempotencyKey::for_operation(
        "texo.code.index.recorded.v1",
        &[payload.workspace_id.as_str(), payload.index_id.as_str()],
    );
    let options = code_index_append_options(op_env, key, payload.snapshot_id.as_str());
    op_env
        .store
        .append_typed_with_options(&coordinate, &payload, options)
        .map_err(Into::into)
}

fn append_source_snapshot_relation(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<SourceSnapshotRelationV1>(payload_bytes)?;
    let coordinate = coordinate_for_source_relation(
        payload.workspace_id.as_str(),
        payload.left_snapshot_id.as_str(),
        payload.right_snapshot_id.as_str(),
    )?;
    let key = IdempotencyKey::for_operation(
        "texo.source.snapshot.relation.v1",
        &[
            payload.workspace_id.as_str(),
            payload.left_snapshot_id.as_str(),
            payload.right_snapshot_id.as_str(),
        ],
    );
    let options = code_index_append_options(op_env, key, payload.right_snapshot_id.as_str());
    op_env
        .store
        .append_typed_with_options(&coordinate, &payload, options)
        .map_err(Into::into)
}

fn append_session_turn(op_env: &OpEnv, payload_bytes: &[u8]) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<SessionTurnV1>(payload_bytes)?;
    let coordinate = coordinate_for_session(&payload.workspace_id, &payload.session_id)?;
    let entity = entity_for_session(&payload.session_id);
    let lane = session_lane(&payload.session_id);
    let depth = u32::try_from(op_env.store.stream_lane(&entity, lane).len()).map_err(|_| {
        TexoError::OpRuntime {
            op: "texo.effect.append".to_string(),
            detail: "session lane depth exceeded u32".to_string(),
            denied: false,
        }
    })?;
    let hint = if depth == 0 {
        AppendPositionHint::branch_root(lane, 0)
    } else {
        AppendPositionHint::new(lane, depth)
    };
    let key = IdempotencyKey::for_operation(
        "texo.session.turn.v1",
        &[
            &payload.workspace_id,
            &payload.session_id,
            &payload.turn_no.to_string(),
        ],
    );
    let options = AppendOptions::new()
        .with_position_hint(hint)
        .with_idempotency(key);
    op_env
        .store
        .append_typed_with_options(&coordinate, &payload, options)
        .map_err(Into::into)
}

fn append_relation_campaign_checkpoint(
    op_env: &OpEnv,
    payload_bytes: &[u8],
) -> Result<AppendReceipt, TexoError> {
    let payload = decode::<RelationCampaignCheckpointV1>(payload_bytes)?;
    if payload.workspace_id.as_str() != op_env.workspace_id.as_str() {
        return Err(TexoError::OpInput {
            op: "texo.effect.append".to_string(),
            detail: "relation campaign checkpoint workspace does not match operation workspace"
                .to_string(),
        });
    }
    for (name, digest) in [
        (
            "evaluated basis",
            payload.evaluated_basis_digest_hex.as_str(),
        ),
        ("result basis", payload.result_basis_digest_hex.as_str()),
        (
            "candidate policy",
            payload.candidate_policy_digest_hex.as_str(),
        ),
    ] {
        if !is_lower_hex_digest(digest) {
            return Err(TexoError::OpInput {
                op: "texo.effect.append".to_string(),
                detail: format!("relation campaign {name} digest must be 64 lowercase hex digits"),
            });
        }
    }
    if matches!(payload.phase, CampaignPhase::Partial { .. })
        && payload.evaluated_basis_digest_hex != payload.result_basis_digest_hex
    {
        return Err(TexoError::OpInput {
            op: "texo.effect.append".to_string(),
            detail: "partial relation campaign checkpoint cannot change the claim basis"
                .to_string(),
        });
    }
    let coordinate = coordinate_for_relation_campaign(payload.workspace_id.as_str())?;
    let options = campaign_checkpoint_options(op_env, &payload)?;
    op_env
        .store
        .append_typed_with_options(&coordinate, &payload, options)
        .map_err(Into::into)
}

fn campaign_checkpoint_options(
    op_env: &OpEnv,
    payload: &RelationCampaignCheckpointV1,
) -> Result<AppendOptions, TexoError> {
    let entity = entity_for_relation_campaign(payload.workspace_id.as_str());
    let latest = op_env
        .store
        .by_entity(&entity)
        .into_iter()
        .max_by_key(batpak::store::IndexEntry::global_sequence);
    let receipt_predecessor = op_env
        .receipts
        .borrow()
        .last()
        .and_then(|receipt| u128::from_str_radix(&receipt.event_id_hex, 16).ok());
    let latest_id = latest.as_ref().map(|entry| entry.event_id().as_u128());
    let retry = receipt_predecessor.is_none_or(|event_id| Some(event_id) == latest_id);
    if let Some(entry) = latest.as_ref().filter(|_| retry) {
        let raw = op_env.store.read_raw(entry.event_id())?;
        let latest = decode::<RelationCampaignCheckpointV1>(&raw.event.payload)?;
        if latest == *payload {
            let key = IdempotencyKey::from(entry.event_id().as_u128());
            return Ok(AppendOptions::new().with_idempotency(key));
        }
    }
    let predecessor = receipt_predecessor.or(latest_id);
    let predecessor_id =
        predecessor.map_or_else(|| "root".to_string(), |event_id| format!("{event_id:032x}"));
    let payload_identity = canonical_payload_identity(payload)?;
    let key = IdempotencyKey::for_operation(
        "texo.relation.campaign.checkpoint.transition.v1",
        &[
            payload.workspace_id.as_str(),
            predecessor_id.as_str(),
            payload_identity.as_str(),
        ],
    );
    let mut options = AppendOptions::new().with_idempotency(key);
    if let Some(event_id) = predecessor {
        options = options.with_causation(CausationId::from(event_id));
    }
    Ok(options)
}

fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn append_with_key<T: EventPayload>(
    op_env: &OpEnv,
    coordinate: &batpak::coordinate::Coordinate,
    payload: &T,
    key: IdempotencyKey,
) -> Result<AppendReceipt, TexoError> {
    op_env
        .store
        .append_typed_with_options(
            coordinate,
            payload,
            AppendOptions::new().with_idempotency(key),
        )
        .map_err(Into::into)
}

fn canonical_payload_identity<T: serde::Serialize>(payload: &T) -> Result<String, TexoError> {
    let bytes = batpak::canonical::to_bytes(payload).map_err(|error| TexoError::OpRuntime {
        op: "texo.effect.append".to_string(),
        detail: format!("canonical effect payload encoding failed: {error}"),
        denied: false,
    })?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn evidence_chain_options(
    op_env: &OpEnv,
    key: IdempotencyKey,
    occurrence_id: &str,
    preferred_cause_kind: EventKind,
) -> Result<AppendOptions, TexoError> {
    let occurrence_entity = entity_for_evidence(occurrence_id);
    let occurrence_entry = op_env
        .store
        .by_entity(&occurrence_entity)
        .into_iter()
        .find(|entry| entry.event_kind() == <EvidenceOccurrenceRecordedV1 as EventPayload>::KIND)
        .ok_or_else(|| TexoError::MissingEntity {
            entity: occurrence_entity.clone(),
        })?;
    let raw = op_env.store.read_raw(occurrence_entry.event_id())?;
    let occurrence = batpak::encoding::from_bytes::<EvidenceOccurrenceRecordedV1>(
        &raw.event.payload,
    )
    .map_err(|error| TexoError::Decode {
        entity: occurrence_entity,
        detail: error.to_string(),
    })?;
    let root = op_env
        .store
        .by_entity(&entity_for_source_snapshot(
            occurrence.occurrence.snapshot_id.as_str(),
        ))
        .into_iter()
        .find(|entry| entry.event_kind() == <SourceSnapshotRecordedV1 as EventPayload>::KIND)
        .map(|entry| entry.event_id().as_u128());
    let receipts = op_env.receipts.borrow();
    let cause = receipts
        .iter()
        .rev()
        .find(|receipt| receipt.kind_bits == preferred_cause_kind.as_raw_u16())
        .and_then(|receipt| u128::from_str_radix(&receipt.event_id_hex, 16).ok())
        .unwrap_or_else(|| occurrence_entry.event_id().as_u128());
    let mut options = AppendOptions::new().with_idempotency(key);
    if let Some(root) = root {
        options = options.with_correlation(CorrelationId::from(root));
    }
    Ok(options.with_causation(CausationId::from(cause)))
}

fn code_index_append_options(
    op_env: &OpEnv,
    key: IdempotencyKey,
    snapshot_id: &str,
) -> AppendOptions {
    let root = op_env
        .store
        .by_entity(&entity_for_source_snapshot(snapshot_id))
        .into_iter()
        .find(|entry| entry.event_kind() == <SourceSnapshotRecordedV1 as EventPayload>::KIND)
        .map(|entry| entry.event_id().as_u128());
    let mut options = AppendOptions::new().with_idempotency(key);
    if let Some(root) = root {
        options = options
            .with_correlation(CorrelationId::from(root))
            .with_causation(CausationId::from(root));
    }
    options
}

fn decode<T>(payload_bytes: &[u8]) -> Result<T, TexoError>
where
    T: serde::de::DeserializeOwned,
{
    batpak::canonical::from_bytes(payload_bytes).map_err(|error| TexoError::OpRuntime {
        op: "texo.effect.append".to_string(),
        detail: format!("canonical effect payload decoding failed: {error}"),
        denied: false,
    })
}
