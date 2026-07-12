//! Texo syncbat effect backend.

use batpak::event::{EventKind, EventPayload};
use batpak::id::{CausationId, CorrelationId, EntityIdType, IdempotencyKey};
use batpak::store::{AppendOptions, AppendPositionHint, AppendReceipt};
use syncbat::{EffectBackend, EffectError};

use crate::error::TexoError;
use crate::events::coordinate::{
    coordinate_for_claim, coordinate_for_code_index, coordinate_for_conflict,
    coordinate_for_evidence, coordinate_for_onboarding_projection, coordinate_for_relation_pair,
    coordinate_for_session, coordinate_for_source, coordinate_for_source_relation,
    coordinate_for_source_snapshot, coordinate_for_workspace_meta, entity_for_evidence,
    entity_for_session, entity_for_source_snapshot, scope_for_workspace, session_lane,
};
use crate::events::ids::relation_pair_id;
use crate::events::machines::{
    ignore_conflict, open_conflict, record_claim, resolve_conflict, supersede_claim,
};
use crate::events::payloads::{
    ClaimEvidenceLinkedV1, ClaimRecordedV2, ClaimSupersededV2, CodeIndexRecordedV1,
    ConflictOpenedV2, ConflictResolvedV2, EvidenceOccurrenceRecordedV1,
    EvidenceReconciliationAcceptedV1, OnboardingCompiledV2, RelationDeferredV1, RelationJudgedV1,
    SessionTurnV1, SourceObservedV2, SourceSnapshotRecordedV1, SourceSnapshotRelationV1,
    WorkspaceInitializedV2,
};
use crate::ops::env::{self, OpEnv, ReceiptNote};

/// Runtime-owned durable effect backend for texo operations.
#[derive(Default)]
pub struct TexoEffectBackend;

impl EffectBackend for TexoEffectBackend {
    fn append_event(&mut self, kind: EventKind, payload: &[u8]) -> Result<(), EffectError> {
        with_env_result(|op_env| append_domain_event(op_env, kind, payload))
    }

    fn read_event(&mut self, _event_category: &str) -> Result<(), EffectError> {
        with_env_result(effect_probe)
    }

    fn query_projection(&mut self, _projection_id: &str) -> Result<(), EffectError> {
        with_env_result(effect_probe)
    }
}

fn effect_probe(op_env: &OpEnv) -> Result<(), TexoError> {
    let scope = scope_for_workspace(&op_env.workspace_id);
    let region = batpak::coordinate::Region::scope(&scope);
    if let Some(entry) = op_env.store.query_entries_after(&region, None, 1).first() {
        let _ = op_env.store.read_raw(entry.event_id())?;
    }
    Ok(())
}

fn with_env_result<T>(f: impl FnOnce(&OpEnv) -> Result<T, TexoError>) -> Result<T, EffectError> {
    env::with(f)
        .map_err(|error| effect_error(&error))?
        .map_err(|error| effect_error(&error))
}

#[expect(
    clippy::too_many_lines,
    reason = "single domain append chokepoint owns all typed decoding, coordinates, and idempotency keys"
)]
fn append_domain_event(
    op_env: &OpEnv,
    kind: EventKind,
    payload_bytes: &[u8],
) -> Result<(), TexoError> {
    if kind == <ClaimRecordedV2 as EventPayload>::KIND {
        let payload = decode::<ClaimRecordedV2>(payload_bytes)?;
        let coordinate = coordinate_for_claim(&payload.workspace_id, &payload.claim_id)?;
        let key = IdempotencyKey::for_operation(
            "texo.claim.recorded.v2",
            &[&payload.workspace_id, &payload.claim_id],
        );
        let payload = record_claim(payload).into_payload();
        let receipt = op_env.store.append_typed_with_options(
            &coordinate,
            &payload,
            AppendOptions::new().with_idempotency(key),
        )?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <ClaimSupersededV2 as EventPayload>::KIND {
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
        let receipt = op_env.store.append_typed_with_options(
            &coordinate,
            &payload,
            AppendOptions::new().with_idempotency(key),
        )?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <ConflictOpenedV2 as EventPayload>::KIND {
        let payload = decode::<ConflictOpenedV2>(payload_bytes)?;
        let coordinate = coordinate_for_conflict(&payload.workspace_id, &payload.conflict_id)?;
        let key = IdempotencyKey::for_operation(
            "texo.conflict.open.v2",
            &[&payload.workspace_id, &payload.conflict_id],
        );
        let payload = open_conflict(payload).into_payload();
        let receipt = op_env.store.append_typed_with_options(
            &coordinate,
            &payload,
            AppendOptions::new().with_idempotency(key),
        )?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <ConflictResolvedV2 as EventPayload>::KIND {
        let payload = decode::<ConflictResolvedV2>(payload_bytes)?;
        let coordinate = coordinate_for_conflict(&payload.workspace_id, &payload.conflict_id)?;
        let key = IdempotencyKey::for_operation(
            "texo.conflict.resolve.v2",
            &[&payload.workspace_id, &payload.conflict_id],
        );
        let receipt = match payload.resolution.as_str() {
            "resolved" => {
                let payload = resolve_conflict(payload).into_payload();
                op_env.store.append_typed_with_options(
                    &coordinate,
                    &payload,
                    AppendOptions::new().with_idempotency(key),
                )?
            }
            "ignored" => {
                let payload = ignore_conflict(payload).into_payload();
                op_env.store.append_typed_with_options(
                    &coordinate,
                    &payload,
                    AppendOptions::new().with_idempotency(key),
                )?
            }
            other => {
                return Err(TexoError::StatusParse {
                    value: other.to_string(),
                });
            }
        };
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <SourceObservedV2 as EventPayload>::KIND {
        let payload = decode::<SourceObservedV2>(payload_bytes)?;
        let coordinate = coordinate_for_source(&payload.workspace_id, &payload.source_id)?;
        let key = IdempotencyKey::for_operation(
            "texo.source.observed.v2",
            &[&payload.workspace_id, &payload.body_hash_hex],
        );
        let receipt = op_env.store.append_typed_with_options(
            &coordinate,
            &payload,
            AppendOptions::new().with_idempotency(key),
        )?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <OnboardingCompiledV2 as EventPayload>::KIND {
        let payload = decode::<OnboardingCompiledV2>(payload_bytes)?;
        let coordinate = coordinate_for_onboarding_projection(&payload.workspace_id)?;
        let receipt = op_env.store.append_typed(&coordinate, &payload)?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <WorkspaceInitializedV2 as EventPayload>::KIND {
        let payload = decode::<WorkspaceInitializedV2>(payload_bytes)?;
        let coordinate = coordinate_for_workspace_meta(&payload.workspace_id)?;
        let key = IdempotencyKey::for_operation(
            "texo.workspace.initialized.v2",
            &[&payload.workspace_id, &payload.config_digest_hex],
        );
        let receipt = op_env.store.append_typed_with_options(
            &coordinate,
            &payload,
            AppendOptions::new().with_idempotency(key),
        )?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <RelationJudgedV1 as EventPayload>::KIND {
        let payload = decode::<RelationJudgedV1>(payload_bytes)?;
        let pair_id = relation_pair_id(
            &payload.workspace_id,
            &payload.older_claim,
            &payload.newer_claim,
        );
        let coordinate =
            coordinate_for_relation_pair(payload.workspace_id.as_str(), pair_id.as_str())?;
        let key = IdempotencyKey::for_operation(
            "texo.relation.judged.v1",
            &[
                payload.workspace_id.as_str(),
                payload.older_claim.as_str(),
                payload.newer_claim.as_str(),
                &payload.judge_fingerprint,
            ],
        );
        let receipt = op_env.store.append_typed_with_options(
            &coordinate,
            &payload,
            AppendOptions::new().with_idempotency(key),
        )?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <RelationDeferredV1 as EventPayload>::KIND {
        let payload = decode::<RelationDeferredV1>(payload_bytes)?;
        let pair_id = relation_pair_id(
            &payload.workspace_id,
            &payload.older_claim,
            &payload.newer_claim,
        );
        let coordinate =
            coordinate_for_relation_pair(payload.workspace_id.as_str(), pair_id.as_str())?;
        let receipt = op_env.store.append_typed(&coordinate, &payload)?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <SourceSnapshotRecordedV1 as EventPayload>::KIND {
        let payload = decode::<SourceSnapshotRecordedV1>(payload_bytes)?;
        let coordinate = coordinate_for_source_snapshot(
            payload.workspace_id.as_str(),
            payload.snapshot_id.as_str(),
        )?;
        let key = IdempotencyKey::for_operation(
            "texo.source.snapshot.recorded.v1",
            &[payload.workspace_id.as_str(), payload.snapshot_id.as_str()],
        );
        let receipt = op_env.store.append_typed_with_options(
            &coordinate,
            &payload,
            AppendOptions::new().with_idempotency(key),
        )?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <EvidenceOccurrenceRecordedV1 as EventPayload>::KIND {
        let payload = decode::<EvidenceOccurrenceRecordedV1>(payload_bytes)?;
        payload
            .occurrence
            .validate()
            .map_err(|error| TexoError::OpInput {
                op: "texo.knowledge.index".to_string(),
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
        let options =
            code_index_append_options(op_env, key, payload.occurrence.snapshot_id.as_str());
        let receipt = op_env
            .store
            .append_typed_with_options(&coordinate, &payload, options)?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <EvidenceReconciliationAcceptedV1 as EventPayload>::KIND {
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
        let receipt = op_env
            .store
            .append_typed_with_options(&coordinate, &payload, options)?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <ClaimEvidenceLinkedV1 as EventPayload>::KIND {
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
        let receipt = op_env
            .store
            .append_typed_with_options(&coordinate, &payload, options)?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <CodeIndexRecordedV1 as EventPayload>::KIND {
        let payload = decode::<CodeIndexRecordedV1>(payload_bytes)?;
        let coordinate =
            coordinate_for_code_index(payload.workspace_id.as_str(), payload.index_id.as_str())?;
        let key = IdempotencyKey::for_operation(
            "texo.code.index.recorded.v1",
            &[payload.workspace_id.as_str(), payload.index_id.as_str()],
        );
        let options = code_index_append_options(op_env, key, payload.snapshot_id.as_str());
        let receipt = op_env
            .store
            .append_typed_with_options(&coordinate, &payload, options)?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <SourceSnapshotRelationV1 as EventPayload>::KIND {
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
        let receipt = op_env
            .store
            .append_typed_with_options(&coordinate, &payload, options)?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <SessionTurnV1 as EventPayload>::KIND {
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
        let receipt = op_env.store.append_typed_with_options(
            &coordinate,
            &payload,
            AppendOptions::new()
                .with_position_hint(hint)
                .with_idempotency(IdempotencyKey::for_operation(
                    "texo.session.turn.v1",
                    &[
                        &payload.workspace_id,
                        &payload.session_id,
                        &payload.turn_no.to_string(),
                    ],
                )),
        )?;
        verify_and_note(op_env, kind, &receipt)
    } else {
        Err(TexoError::OpRuntime {
            op: "texo.effect.append".to_string(),
            detail: format!(
                "event kind evt.{:04x} is outside texo domain",
                kind.as_raw_u16()
            ),
            denied: false,
        })
    }
}

fn evidence_chain_options(
    op_env: &OpEnv,
    key: IdempotencyKey,
    occurrence_id: &str,
    preferred_cause_kind: EventKind,
) -> Result<AppendOptions, TexoError> {
    let occurrence_entry = op_env
        .store
        .by_entity(&entity_for_evidence(occurrence_id))
        .into_iter()
        .find(|entry| entry.event_kind() == <EvidenceOccurrenceRecordedV1 as EventPayload>::KIND)
        .ok_or_else(|| TexoError::MissingEntity {
            entity: entity_for_evidence(occurrence_id),
        })?;
    let raw = op_env.store.read_raw(occurrence_entry.event_id())?;
    let occurrence = batpak::encoding::from_bytes::<EvidenceOccurrenceRecordedV1>(
        &raw.event.payload,
    )
    .map_err(|error| TexoError::Decode {
        entity: entity_for_evidence(occurrence_id),
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
    options = options.with_causation(CausationId::from(cause));
    Ok(options)
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
    serde_json::from_slice(payload_bytes).map_err(TexoError::Json)
}

fn verify_and_note(
    op_env: &OpEnv,
    kind: EventKind,
    receipt: &AppendReceipt,
) -> Result<(), TexoError> {
    let verification = op_env.store.verify_append_receipt(receipt);
    if !verification.is_valid() {
        return Err(TexoError::ReceiptInvalid {
            event_id: event_id_hex(receipt.event_id),
            reason: verification.error().map_or_else(
                || "invalid receipt".to_string(),
                |error| format!("{error:?}"),
            ),
        });
    }

    op_env.receipts.borrow_mut().push(ReceiptNote {
        event_id_hex: event_id_hex(receipt.event_id),
        kind_bits: kind.as_raw_u16(),
        global_sequence: receipt.global_sequence,
    });
    Ok(())
}

fn event_id_hex(event_id: batpak::id::EventId) -> String {
    format!("{:032x}", event_id.as_u128())
}

impl From<batpak::coordinate::CoordinateError> for TexoError {
    fn from(error: batpak::coordinate::CoordinateError) -> Self {
        Self::Coordinate {
            detail: error.to_string(),
        }
    }
}

fn effect_error(error: &TexoError) -> EffectError {
    EffectError::new(format!("{}: {error}", error.code()))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::Arc;

    use batpak::store::{Store, StoreConfig};

    use super::*;
    use crate::claims::workspace::WorkspaceCache;
    use crate::config::WorkspaceConfig;
    use crate::events::coordinate::coordinate_for_claim;
    use crate::events::ids::{ClaimId, WorkspaceId};
    use crate::events::machines::record_claim;
    use crate::relate::settlement::SettledRelation;

    #[test]
    fn keyed_source_retry_returns_original_event_and_appends_once() {
        let root = tempfile::tempdir().expect("tempdir");
        let store =
            Arc::new(Store::open(StoreConfig::new(root.path().join("store"))).expect("store"));
        let env = OpEnv {
            store: Arc::clone(&store),
            workspace_id: "demo".to_string(),
            root: root.path().to_path_buf(),
            config: WorkspaceConfig::demo(),
            cache: RefCell::new(WorkspaceCache::default()),
            receipts: RefCell::new(Vec::new()),
            observed_at_ms: 1,
        };
        let payload = SourceObservedV2 {
            source_id: "src_aaaaaaaaaaaa".to_string(),
            workspace_id: "demo".to_string(),
            source_kind: "markdown".to_string(),
            path: "docs/a.md".to_string(),
            body_hash_hex: "body".to_string(),
            observed_at_ms: 1,
        };
        let bytes = serde_json::to_vec(&payload).expect("json");
        append_domain_event(&env, <SourceObservedV2 as EventPayload>::KIND, &bytes).expect("first");
        append_domain_event(&env, <SourceObservedV2 as EventPayload>::KIND, &bytes).expect("retry");

        assert_eq!(store.by_entity("source:src_aaaaaaaaaaaa").len(), 1);
        let receipts = env.receipts.borrow();
        assert_eq!(receipts.len(), 2);
        assert_eq!(receipts[0], receipts[1]);
        let expected = IdempotencyKey::for_operation("texo.source.observed.v2", &["demo", "body"]);
        assert_eq!(
            receipts[0].event_id_hex,
            format!("{:032x}", expected.as_u128())
        );
    }

    #[test]
    fn keyed_judgment_retry_returns_original_event_id() {
        let root = tempfile::tempdir().expect("tempdir");
        let store =
            Arc::new(Store::open(StoreConfig::new(root.path().join("store"))).expect("store"));
        let env = OpEnv {
            store: Arc::clone(&store),
            workspace_id: "demo".to_string(),
            root: root.path().to_path_buf(),
            config: WorkspaceConfig::demo(),
            cache: RefCell::new(WorkspaceCache::default()),
            receipts: RefCell::new(Vec::new()),
            observed_at_ms: 1,
        };
        let payload = RelationJudgedV1 {
            workspace_id: WorkspaceId::try_from("demo").expect("workspace"),
            older_claim: ClaimId::try_from("claim_aaaaaaaaaaaa").expect("older"),
            newer_claim: ClaimId::try_from("claim_bbbbbbbbbbbb").expect("newer"),
            relation: SettledRelation::Supersedes,
            score_ppm: 900_000,
            judge_fingerprint: "openrouter:model|relation-v2".to_string(),
            cache_key_hex: "cache".to_string(),
            observed_at_ms: 1,
        };
        let bytes = serde_json::to_vec(&payload).expect("json");
        append_domain_event(&env, <RelationJudgedV1 as EventPayload>::KIND, &bytes).expect("first");
        append_domain_event(&env, <RelationJudgedV1 as EventPayload>::KIND, &bytes).expect("retry");

        let expected = IdempotencyKey::for_operation(
            "texo.relation.judged.v1",
            &[
                "demo",
                "claim_aaaaaaaaaaaa",
                "claim_bbbbbbbbbbbb",
                "openrouter:model|relation-v2",
            ],
        );
        let receipts = env.receipts.borrow();
        assert_eq!(receipts.len(), 2);
        assert_eq!(receipts[0], receipts[1]);
        assert_eq!(
            receipts[0].event_id_hex,
            format!("{:032x}", expected.as_u128())
        );
        let pair_id = relation_pair_id(
            &payload.workspace_id,
            &payload.older_claim,
            &payload.newer_claim,
        );
        assert_eq!(store.by_entity(&format!("relation:{pair_id}")).len(), 1);
    }

    #[test]
    fn typed_options_transition_preserves_payload_bytes() {
        let left_dir = tempfile::tempdir().expect("left tempdir");
        let right_dir = tempfile::tempdir().expect("right tempdir");
        let left = Store::open(StoreConfig::new(left_dir.path())).expect("left store");
        let right = Store::open(StoreConfig::new(right_dir.path())).expect("right store");
        let coordinate = coordinate_for_claim("demo", "claim_aaaaaaaaaaaa").expect("coordinate");
        let payload = ClaimRecordedV2 {
            claim_id: "claim_aaaaaaaaaaaa".to_string(),
            workspace_id: "demo".to_string(),
            source_id: "src_aaaaaaaaaaaa".to_string(),
            source_path: "docs/a.md".to_string(),
            line_start: 1,
            line_end: 1,
            char_start: 0,
            char_end: 5,
            text: "claim".to_string(),
            normalized_text: "claim".to_string(),
            subject_hint: None,
            predicate_hint: None,
            object_hint: None,
            confidence_ppm: 1_000_000,
            extractor_kind: "test".to_string(),
            extractor_model: String::new(),
            prompt_version: String::new(),
            observed_at_ms: 1,
        };
        let left_receipt = left
            .apply_transition(&coordinate, record_claim(payload.clone()))
            .expect("transition");
        let typed_payload = record_claim(payload).into_payload();
        let right_receipt = right
            .append_typed_with_options(
                &coordinate,
                &typed_payload,
                AppendOptions::new().with_idempotency(IdempotencyKey::for_operation(
                    "test",
                    &["claim_aaaaaaaaaaaa"],
                )),
            )
            .expect("typed options");
        let left_raw = left.read_raw(left_receipt.event_id).expect("left raw");
        let right_raw = right.read_raw(right_receipt.event_id).expect("right raw");
        assert_eq!(
            left_raw.event.header.event_kind,
            right_raw.event.header.event_kind
        );
        assert_eq!(left_raw.event.payload, right_raw.event.payload);
    }
}
