//! Texo syncbat effect backend.

use batpak::event::{EventKind, EventPayload};
use batpak::id::EntityIdType;
use batpak::store::{AppendOptions, AppendPositionHint, AppendReceipt};
use syncbat::{EffectBackend, EffectError};

use crate::claims::workspace::assemble;
use crate::error::TexoError;
use crate::events::coordinate::{
    coordinate_for_claim, coordinate_for_conflict, coordinate_for_onboarding_projection,
    coordinate_for_session, coordinate_for_source, coordinate_for_workspace_meta,
    entity_for_session, scope_for_workspace, session_lane,
};
use crate::events::machines::{
    ignore_conflict, open_conflict, record_claim, resolve_conflict, supersede_claim,
};
use crate::events::payloads::{
    ClaimRecordedV2, ClaimSupersededV2, ConflictOpenedV2, ConflictResolvedV2, OnboardingCompiledV2,
    SessionTurnV1, SourceObservedV2, WorkspaceInitializedV2,
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
        with_env_result(|op_env| {
            let scope = scope_for_workspace(&op_env.workspace_id);
            drop(op_env.store.by_scope(&scope));
            Ok(())
        })
    }

    fn query_projection(&mut self, _projection_id: &str) -> Result<(), EffectError> {
        with_env_result(|op_env| {
            let mut cache = op_env.cache.borrow_mut();
            drop(assemble(&op_env.store, &op_env.workspace_id, &mut cache)?);
            Ok(())
        })
    }
}

fn with_env_result<T>(f: impl FnOnce(&OpEnv) -> Result<T, TexoError>) -> Result<T, EffectError> {
    env::with(f)
        .map_err(|error| effect_error(&error))?
        .map_err(|error| effect_error(&error))
}

fn append_domain_event(
    op_env: &OpEnv,
    kind: EventKind,
    payload_bytes: &[u8],
) -> Result<(), TexoError> {
    if kind == <ClaimRecordedV2 as EventPayload>::KIND {
        let payload = decode::<ClaimRecordedV2>(payload_bytes)?;
        let coordinate = coordinate_for_claim(&payload.workspace_id, &payload.claim_id)?;
        let receipt = op_env
            .store
            .apply_transition(&coordinate, record_claim(payload))?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <ClaimSupersededV2 as EventPayload>::KIND {
        let payload = decode::<ClaimSupersededV2>(payload_bytes)?;
        let coordinate = coordinate_for_claim(&payload.workspace_id, &payload.old_claim_id)?;
        let receipt = op_env
            .store
            .apply_transition(&coordinate, supersede_claim(payload))?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <ConflictOpenedV2 as EventPayload>::KIND {
        let payload = decode::<ConflictOpenedV2>(payload_bytes)?;
        let coordinate = coordinate_for_conflict(&payload.workspace_id, &payload.conflict_id)?;
        let receipt = op_env
            .store
            .apply_transition(&coordinate, open_conflict(payload))?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <ConflictResolvedV2 as EventPayload>::KIND {
        let payload = decode::<ConflictResolvedV2>(payload_bytes)?;
        let coordinate = coordinate_for_conflict(&payload.workspace_id, &payload.conflict_id)?;
        let receipt = match payload.resolution.as_str() {
            "resolved" => op_env
                .store
                .apply_transition(&coordinate, resolve_conflict(payload))?,
            "ignored" => op_env
                .store
                .apply_transition(&coordinate, ignore_conflict(payload))?,
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
        let receipt = op_env.store.append_typed(&coordinate, &payload)?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <OnboardingCompiledV2 as EventPayload>::KIND {
        let payload = decode::<OnboardingCompiledV2>(payload_bytes)?;
        let coordinate = coordinate_for_onboarding_projection(&payload.workspace_id)?;
        let receipt = op_env.store.append_typed(&coordinate, &payload)?;
        verify_and_note(op_env, kind, &receipt)
    } else if kind == <WorkspaceInitializedV2 as EventPayload>::KIND {
        let payload = decode::<WorkspaceInitializedV2>(payload_bytes)?;
        let coordinate = coordinate_for_workspace_meta(&payload.workspace_id)?;
        let receipt = op_env.store.append_typed(&coordinate, &payload)?;
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
            AppendOptions::new().with_position_hint(hint),
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
