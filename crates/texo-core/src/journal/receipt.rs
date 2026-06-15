//! Verify BatPak append receipts and map to texo receipt views.

use std::collections::BTreeMap;

use batpak::prelude::*;

use crate::journal::JournalError;
use crate::types::receipt::ReceiptView;

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
    Ok(ReceiptView::from_verified_append(
        receipt, kind, scope, entity,
    ))
}

/// Re-verify a previously projected [`ReceiptView`] against the live store
/// index.
///
/// Guarantee (current): an existence + sequence + content-hash consistency
/// check on the UNSIGNED path. The fields taken from the view are the
/// `event_id` (existence: the store must contain it) and `sequence`. The
/// content hash is read from the freshly fetched `StoredEvent` header and
/// checked by the store against its own hash chain — a store-internal
/// consistency assertion, not a value re-derived from the view (the view
/// carries no content hash). Verification rebuilds an unsigned wire receipt
/// (`key_id` all zeros, no `signature`, no `extensions`) and asks the store to
/// verify it; on a keyless store this resolves to `UnsignedAccepted`.
///
/// Limitation (not yet wired): true signed re-verification. The original
/// [`AppendReceipt`] signing material (`key_id`, `signature`, `extensions`) is
/// ephemeral and is not persisted by BatPak's `StoredEvent`/`EventHeader`, so
/// it cannot be recovered at replay time. Until BatPak persists that material
/// (or texo records it out of band), this function CANNOT re-verify a real
/// signature; on a signing-enabled store it would only assert the unsigned
/// consistency invariant, not the signature itself.
///
/// This call deliberately lives inside the `journal/` module so the
/// re-verification path (`conflicts::verify`) does not invoke BatPak's receipt
/// verification API directly. (`conflicts::verify` still references BatPak
/// `Store`/`Open` types to thread the store handle; that residual type import
/// is expected.)
pub fn verify_receipt_view(store: &Store<Open>, receipt: &ReceiptView) -> Result<(), JournalError> {
    let event_id_hex = receipt.event_id.as_str();
    let raw = event_id_hex.strip_prefix("0x").unwrap_or(event_id_hex);
    let event_id = EventId::from(
        u128::from_str_radix(raw, 16)
            .map_err(|e| JournalError::ReceiptInvalid(format!("invalid event id: {e}")))?,
    );
    let stored = store
        .get(event_id)
        .map_err(|e| JournalError::ReceiptInvalid(format!("store get: {e}")))?;
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
        return Err(JournalError::ReceiptInvalid(format!(
            "receipt {} invalid: {message}",
            receipt.event_id
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::payloads::SourceObserved;
    use crate::journal::append::append_source_observed;
    use crate::journal::store::StoreHandle;
    use crate::types::ids::WorkspaceId;
    use crate::types::sequence::LocalSequence;

    fn append_one_source(store: &Store<Open>, workspace: &WorkspaceId) -> ReceiptView {
        let payload = SourceObserved {
            source_id: "src-1".to_string(),
            workspace_id: workspace.as_str().to_string(),
            source_kind: "markdown".to_string(),
            path: "notes.md".to_string(),
            body_hash_hex: "00".repeat(32),
            observed_at_ms: 1,
        };
        append_source_observed(store, workspace, &payload).expect("append source observed")
    }

    #[test]
    fn verify_receipt_view_accepts_valid_unsigned_receipt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handle = StoreHandle::open(dir.path()).expect("open store");
        let workspace = WorkspaceId::new("demo").expect("workspace id");

        let view = append_one_source(handle.store(), &workspace);
        verify_receipt_view(handle.store(), &view).expect("valid unsigned receipt accepted");

        handle.close().expect("close store");
    }

    #[test]
    fn verify_receipt_view_rejects_sequence_tamper() {
        // The unsigned consistency check must still catch a receipt whose
        // claimed sequence does not match the committed event. This proves the
        // existence + sequence + content-hash guarantee is real, even though
        // signed re-verification is not yet wired.
        let dir = tempfile::tempdir().expect("tempdir");
        let handle = StoreHandle::open(dir.path()).expect("open store");
        let workspace = WorkspaceId::new("demo").expect("workspace id");

        let mut view = append_one_source(handle.store(), &workspace);
        view.sequence = LocalSequence::new(view.sequence.get().wrapping_add(1));

        let err = verify_receipt_view(handle.store(), &view)
            .expect_err("tampered sequence must be rejected");
        match err {
            JournalError::ReceiptInvalid(_) => {}
            other => panic!("expected ReceiptInvalid, got {other:?}"),
        }

        handle.close().expect("close store");
    }
}
