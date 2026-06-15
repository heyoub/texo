//! Journal receipt verification against BatPak store index.

use batpak::prelude::{Open, Store};

use crate::events::envelope::TexoEvent;
use crate::journal::receipt::verify_receipt_view;
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
    verify_receipt_view(store, receipt).map_err(|e| VerifyError::Journal(e.to_string()))
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
