//! Paginated journal replay and lightweight source scans.

use std::collections::HashSet;

use batpak::prelude::*;

use crate::events::envelope::{DecodeError, TexoEvent};
use crate::events::payloads::{
    ClaimConflictDetected, ClaimRecorded, ClaimSuperseded, OnboardingCompiled, SourceObserved,
};
use crate::types::ids::WorkspaceId;
use crate::types::receipt::receipt_view;

const PAGE_SIZE: usize = 256;

/// Load all texo events for a workspace in commit order.
pub fn load_workspace_events(
    store: &Store<Open>,
    workspace: &WorkspaceId,
) -> Result<Vec<TexoEvent>, DecodeError> {
    let region = Region::scope(workspace.scope());
    let mut after: Option<u64> = None;
    let mut events = Vec::new();

    loop {
        let page = store.query_entries_after(&region, after, PAGE_SIZE);
        if page.is_empty() {
            break;
        }
        for entry in &page {
            let event_id = EventId::from(entry.event_id());
            let stored = store
                .get(event_id)
                .map_err(|e| DecodeError::Decode(e.to_string()))?;
            if let Some(decoded) = decode_stored_event(&stored, entry.global_sequence())? {
                events.push(decoded);
            }
        }
        after = Some(page.last().expect("non-empty page").global_sequence());
    }

    Ok(events)
}

/// Collect source body hashes without full claim replay.
pub fn load_source_body_hashes(
    store: &Store<Open>,
    workspace: &WorkspaceId,
) -> Result<HashSet<String>, DecodeError> {
    let region = Region::scope(workspace.scope());
    let mut after: Option<u64> = None;
    let mut hashes = HashSet::new();

    loop {
        let page = store.query_entries_after(&region, after, PAGE_SIZE);
        if page.is_empty() {
            break;
        }
        for entry in &page {
            let event_id = EventId::from(entry.event_id());
            let stored = store
                .get(event_id)
                .map_err(|e| DecodeError::Decode(e.to_string()))?;
            if let Some(payload) = stored
                .event
                .route_typed::<SourceObserved>()
                .map_err(map_typed_decode_error)?
            {
                hashes.insert(payload.body_hash_hex);
            }
        }
        after = Some(page.last().expect("non-empty page").global_sequence());
    }

    Ok(hashes)
}

fn map_typed_decode_error(error: TypedDecodeError) -> DecodeError {
    match error {
        TypedDecodeError::KindMismatch { .. } => DecodeError::UnsupportedKind,
        TypedDecodeError::DecodeFailure { source, .. } => DecodeError::Decode(source.to_string()),
    }
}

fn decode_stored_event(
    stored: &StoredEvent<serde_json::Value>,
    sequence: u64,
) -> Result<Option<TexoEvent>, DecodeError> {
    let scope = stored.coordinate.scope().to_string();
    let entity = stored.coordinate.entity().to_string();
    let event_id = stored.event.header.event_id;
    let base_receipt = receipt_view(event_id.into(), sequence, "", &scope, &entity);

    if let Some(payload) = stored
        .event
        .route_typed::<SourceObserved>()
        .map_err(map_typed_decode_error)?
    {
        let mut receipt = base_receipt;
        receipt.kind = "SourceObserved".to_string();
        return Ok(Some(TexoEvent::SourceObserved { payload, receipt }));
    }
    if let Some(payload) = stored
        .event
        .route_typed::<ClaimRecorded>()
        .map_err(map_typed_decode_error)?
    {
        let mut receipt = base_receipt;
        receipt.kind = "ClaimRecorded".to_string();
        return Ok(Some(TexoEvent::ClaimRecorded { payload, receipt }));
    }
    if let Some(payload) = stored
        .event
        .route_typed::<ClaimSuperseded>()
        .map_err(map_typed_decode_error)?
    {
        let mut receipt = base_receipt;
        receipt.kind = "ClaimSuperseded".to_string();
        return Ok(Some(TexoEvent::ClaimSuperseded { payload, receipt }));
    }
    if let Some(payload) = stored
        .event
        .route_typed::<ClaimConflictDetected>()
        .map_err(map_typed_decode_error)?
    {
        let mut receipt = base_receipt;
        receipt.kind = "ClaimConflictDetected".to_string();
        return Ok(Some(TexoEvent::ClaimConflictDetected { payload, receipt }));
    }
    if let Some(payload) = stored
        .event
        .route_typed::<OnboardingCompiled>()
        .map_err(map_typed_decode_error)?
    {
        let mut receipt = base_receipt;
        receipt.kind = "OnboardingCompiled".to_string();
        return Ok(Some(TexoEvent::OnboardingCompiled { payload, receipt }));
    }

    Ok(None)
}
