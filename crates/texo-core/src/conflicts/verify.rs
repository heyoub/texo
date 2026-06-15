//! Journal receipt verification against BatPak store index.

use std::collections::BTreeMap;

use batpak::prelude::*;

use crate::events::envelope::TexoEvent;
use crate::journal::replay::load_workspace_events;
use crate::types::ids::WorkspaceId;

use super::detect::VerifyError;

/// Verify all texo event receipts in a workspace against the live store index.
pub fn verify_journal_receipts(
    store: &Store<Open>,
    workspace: &WorkspaceId,
) -> Result<(), VerifyError> {
    let events =
        load_workspace_events(store, workspace).map_err(|e| VerifyError::Journal(e.to_string()))?;
    for event in events {
        verify_event_receipt(store, &event)?;
    }
    Ok(())
}

fn verify_event_receipt(store: &Store<Open>, event: &TexoEvent) -> Result<(), VerifyError> {
    let receipt = event_receipt_view(event);
    let event_id_hex = receipt.event_id.as_str();
    let raw = event_id_hex.strip_prefix("0x").unwrap_or(event_id_hex);
    let event_id = EventId::from(
        u128::from_str_radix(raw, 16)
            .map_err(|e| VerifyError::Journal(format!("invalid event id: {e}")))?,
    );
    let stored = store
        .get(event_id)
        .map_err(|e| VerifyError::Journal(format!("store get: {e}")))?;
    let verification = store.verify_append_receipt_wire_detailed(
        event_id,
        receipt.sequence.get(),
        stored.event.header.content_hash,
        [0u8; 32],
        None,
        BTreeMap::new(),
    );
    if !verification.is_valid() {
        let message = verification
            .error()
            .map_or_else(|| "unknown".to_string(), |e| format!("{e:?}"));
        return Err(VerifyError::Journal(format!(
            "receipt {} invalid: {message}",
            receipt.event_id
        )));
    }
    Ok(())
}

fn event_receipt_view(event: &TexoEvent) -> &crate::types::receipt::ReceiptView {
    match event {
        TexoEvent::SourceObserved { receipt, .. }
        | TexoEvent::ClaimRecorded { receipt, .. }
        | TexoEvent::ClaimSuperseded { receipt, .. }
        | TexoEvent::ClaimConflictDetected { receipt, .. }
        | TexoEvent::OnboardingCompiled { receipt, .. } => receipt,
    }
}
