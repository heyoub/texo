//! Verify BatPak append receipts and map to texo receipt views.

use batpak::prelude::*;

use crate::journal::JournalError;
use crate::types::receipt::{receipt_view, ReceiptView};

/// Verify an append receipt against the store, then build a JSON-facing view.
pub fn verify_and_view(
    store: &Store<Open>,
    receipt: &AppendReceipt,
    kind: &str,
    scope: &str,
    entity: &str,
) -> Result<ReceiptView, JournalError> {
    let verification = store.verify_append_receipt_detailed(receipt);
    if !verification.is_valid() {
        let message = verification.error().map_or_else(
            || "unknown receipt verification failure".to_string(),
            |e| format!("{e:?}"),
        );
        return Err(JournalError::ReceiptInvalid(message));
    }
    Ok(receipt_view(
        receipt.event_id.into(),
        receipt.sequence,
        kind,
        scope,
        entity,
    ))
}
