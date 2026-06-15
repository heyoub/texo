//! Typed append helpers.

use batpak::prelude::*;

use crate::events::payloads::{
    ClaimConflictDetected, ClaimRecorded, ClaimSuperseded, OnboardingCompiled, SourceObserved,
};
use crate::journal::receipt::verify_and_view;
use crate::journal::JournalError;
use crate::types::coordinate::{
    entity_for_claim, entity_for_conflict, entity_for_projection, entity_for_source,
    scope_for_workspace,
};
use crate::types::ids::WorkspaceId;
use crate::types::receipt::ReceiptView;

/// Append `SourceObserved` to the source entity stream.
pub fn append_source_observed(
    store: &Store<Open>,
    workspace: &WorkspaceId,
    payload: &SourceObserved,
) -> Result<ReceiptView, JournalError> {
    let scope = scope_for_workspace(workspace.as_str());
    let entity = entity_for_source(&payload.source_id);
    let coord = Coordinate::new(entity.clone(), scope.clone())?;
    let receipt = store.append_typed(&coord, payload)?;
    verify_and_view(store, &receipt, "SourceObserved", &scope, &entity)
}

/// Append `ClaimRecorded` to the claim entity stream.
pub fn append_claim_recorded(
    store: &Store<Open>,
    payload: &ClaimRecorded,
) -> Result<ReceiptView, JournalError> {
    let scope = scope_for_workspace(&payload.workspace_id);
    let entity = entity_for_claim(&payload.claim_id);
    let coord = Coordinate::new(entity.clone(), scope.clone())?;
    let receipt = store.append_typed(&coord, payload)?;
    verify_and_view(store, &receipt, "ClaimRecorded", &scope, &entity)
}

/// Append `ClaimSuperseded` to the old claim entity stream.
pub fn append_superseded(
    store: &Store<Open>,
    payload: &ClaimSuperseded,
) -> Result<ReceiptView, JournalError> {
    let scope = scope_for_workspace(&payload.workspace_id);
    let entity = entity_for_claim(&payload.old_claim_id);
    let coord = Coordinate::new(entity.clone(), scope.clone())?;
    let receipt = store.append_typed(&coord, payload)?;
    verify_and_view(store, &receipt, "ClaimSuperseded", &scope, &entity)
}

/// Append `ClaimConflictDetected`.
pub fn append_conflict(
    store: &Store<Open>,
    payload: &ClaimConflictDetected,
) -> Result<ReceiptView, JournalError> {
    let scope = scope_for_workspace(&payload.workspace_id);
    let entity = entity_for_conflict(&payload.conflict_id);
    let coord = Coordinate::new(entity.clone(), scope.clone())?;
    let receipt = store.append_typed(&coord, payload)?;
    verify_and_view(store, &receipt, "ClaimConflictDetected", &scope, &entity)
}

/// Append `OnboardingCompiled` to the projection stream.
pub fn append_onboarding_compiled(
    store: &Store<Open>,
    payload: &OnboardingCompiled,
) -> Result<ReceiptView, JournalError> {
    let scope = scope_for_workspace(&payload.workspace_id);
    let entity = entity_for_projection("onboarding");
    let coord = Coordinate::new(entity.clone(), scope.clone())?;
    let receipt = store.append_typed(&coord, payload)?;
    verify_and_view(store, &receipt, "OnboardingCompiled", &scope, &entity)
}
