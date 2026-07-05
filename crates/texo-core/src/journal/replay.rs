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
        // An empty page ends pagination; a non-empty page's last entry advances
        // the cursor. Binding `last` here makes the non-empty invariant
        // structural — no `.last().expect(...)` is needed.
        let Some(last) = page.last() else { break };
        after = Some(last.global_sequence());
        for entry in &page {
            let event_id = entry.event_id();
            let stored = store.get(event_id)?;
            // Strict policy: an unknown kind propagates as an error here rather
            // than being silently skipped (SPEC.md:73 — no silent partial state).
            events.push(decode_stored_event(&stored, entry.global_sequence())?);
        }
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
        // See `load_workspace_events`: binding the last entry expresses the
        // non-empty invariant structurally instead of asserting it.
        let Some(last) = page.last() else { break };
        after = Some(last.global_sequence());
        for entry in &page {
            let event_id = entry.event_id();
            let stored = store.get(event_id)?;
            // Decode against all five known kinds so a truly unknown kind still
            // errors (no silent skip), then filter to the SOURCE kind we want.
            // Other KNOWN kinds are intentionally ignored here.
            if let TexoEvent::SourceObserved { payload, .. } =
                decode_stored_event(&stored, entry.global_sequence())?
            {
                hashes.insert(payload.body_hash_hex);
            }
        }
    }

    Ok(hashes)
}

/// Decode one stored entry into a known texo event.
///
/// Recognizes all five known texo kinds. A kind that matches none of them is
/// treated as an error (`DecodeError::UnsupportedKind`) rather than being
/// silently skipped, so callers replay full state or fail loudly.
fn decode_stored_event(
    stored: &StoredEvent<serde_json::Value>,
    sequence: u64,
) -> Result<TexoEvent, DecodeError> {
    let scope = stored.coordinate.scope().to_string();
    let entity = stored.coordinate.entity().to_string();
    let event_id = stored.event.header.event_id;
    let base_receipt = receipt_view(event_id.into(), sequence, "", &scope, &entity);

    if let Some(payload) = stored.event.route_typed::<SourceObserved>()? {
        let mut receipt = base_receipt;
        receipt.kind = "SourceObserved".to_string();
        return Ok(TexoEvent::SourceObserved { payload, receipt });
    }
    if let Some(payload) = stored.event.route_typed::<ClaimRecorded>()? {
        let mut receipt = base_receipt;
        receipt.kind = "ClaimRecorded".to_string();
        return Ok(TexoEvent::ClaimRecorded { payload, receipt });
    }
    if let Some(payload) = stored.event.route_typed::<ClaimSuperseded>()? {
        let mut receipt = base_receipt;
        receipt.kind = "ClaimSuperseded".to_string();
        return Ok(TexoEvent::ClaimSuperseded { payload, receipt });
    }
    if let Some(payload) = stored.event.route_typed::<ClaimConflictDetected>()? {
        let mut receipt = base_receipt;
        receipt.kind = "ClaimConflictDetected".to_string();
        return Ok(TexoEvent::ClaimConflictDetected { payload, receipt });
    }
    if let Some(payload) = stored.event.route_typed::<OnboardingCompiled>()? {
        let mut receipt = base_receipt;
        receipt.kind = "OnboardingCompiled".to_string();
        return Ok(TexoEvent::OnboardingCompiled { payload, receipt });
    }

    // None of the five known kinds matched: this is a truly unknown kind, which
    // must not be silently dropped (SPEC.md:73 — no silent partial state).
    Err(DecodeError::UnsupportedKind)
}
