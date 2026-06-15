//! BatPak store handle — sole BatPak import site.

use std::collections::HashSet;
use std::path::Path;

use batpak::prelude::*;

use crate::config::WorkspaceConfig;
use crate::events::envelope::{DecodeError, TexoEvent};
use crate::events::payloads::{
    ClaimConflictDetected, ClaimRecorded, ClaimSuperseded, OnboardingCompiled, SourceObserved,
};
use crate::ingest::PlannedAction;
use crate::journal::replay::{load_source_body_hashes, load_workspace_events};
use crate::replay::reducer::ReplayedState;
use crate::replay::state::ClaimView;
use crate::replay::ReplayError;
use crate::state::ingest::{IngestCommitted, IngestMode, IngestPlan};
use crate::types::ids::{ClaimId, WorkspaceId};
use crate::types::receipt::ReceiptView;
use crate::types::status::ClaimStatus;

/// Journal-specific errors.
#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    /// BatPak store error.
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// Coordinate validation error.
    #[error("coordinate: {0}")]
    Coordinate(#[from] CoordinateError),
    /// Event decode error.
    #[error("decode: {0}")]
    Decode(#[from] DecodeError),
    /// Replay projection error.
    #[error("replay: {0}")]
    Replay(#[from] ReplayError),
    /// Append receipt failed BatPak verification.
    #[error("receipt invalid: {0}")]
    ReceiptInvalid(String),
    /// Domain validation error.
    #[error("{0}")]
    Domain(String),
}

/// Active claim candidates and already-recorded supersession edges for incremental
/// supersession inference.
type SupersessionContext = (Vec<(ClaimId, ClaimView)>, HashSet<(String, String)>);

/// Owned BatPak store handle (`Store<Open>`).
pub struct StoreHandle {
    store: Store<Open>,
}

impl StoreHandle {
    /// Open a store at the given directory path.
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        validate_event_payload_registry()
            .map_err(|e| JournalError::Domain(format!("event payload registry: {e}")))?;
        let store = Store::open(StoreConfig::new(path))?;
        Ok(Self { store })
    }

    /// Borrow the underlying BatPak store.
    pub fn store(&self) -> &Store<Open> {
        &self.store
    }

    /// Close the store cleanly.
    pub fn close(self) -> Result<(), JournalError> {
        self.store.close()?;
        Ok(())
    }

    /// Replay all texo events for a workspace.
    pub fn replay_workspace(
        &self,
        workspace: &WorkspaceId,
        _config: &WorkspaceConfig,
    ) -> Result<ReplayedState, JournalError> {
        let events = load_workspace_events(&self.store, workspace)?;
        Ok(ReplayedState::from_events(events)?)
    }

    /// Append a source observed event.
    pub fn append_source(
        &self,
        workspace: &WorkspaceId,
        payload: &SourceObserved,
    ) -> Result<ReceiptView, JournalError> {
        crate::journal::append::append_source_observed(&self.store, workspace, payload)
    }

    /// Append a claim recorded event.
    pub fn append_claim(&self, payload: &ClaimRecorded) -> Result<ReceiptView, JournalError> {
        crate::journal::append::append_claim_recorded(&self.store, payload)
    }

    /// Append a supersession event.
    pub fn append_superseded(
        &self,
        payload: &ClaimSuperseded,
    ) -> Result<ReceiptView, JournalError> {
        crate::journal::append::append_superseded(&self.store, payload)
    }

    /// Append a conflict event.
    pub fn append_conflict(
        &self,
        payload: &ClaimConflictDetected,
    ) -> Result<ReceiptView, JournalError> {
        crate::journal::append::append_conflict(&self.store, payload)
    }

    /// Append onboarding compiled event.
    pub fn append_onboarding_compiled(
        &self,
        payload: &OnboardingCompiled,
    ) -> Result<ReceiptView, JournalError> {
        crate::journal::append::append_onboarding_compiled(&self.store, payload)
    }

    /// List raw events for tests.
    pub fn load_events(&self, workspace: &WorkspaceId) -> Result<Vec<TexoEvent>, JournalError> {
        Ok(load_workspace_events(&self.store, workspace)?)
    }

    /// Collect known source body hashes without full claim replay.
    pub fn existing_source_hashes(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<HashSet<String>, JournalError> {
        Ok(load_source_body_hashes(&self.store, workspace)?)
    }

    /// Load the workspace's currently-active claims plus already-recorded supersession
    /// edges by replaying the journal.
    ///
    /// Active claims are returned as `(ClaimId, ClaimView)` candidates for incremental
    /// supersession inference; edges are `(old_claim_id, new_claim_id)` pairs that already
    /// exist so they are not re-emitted. Reuses the existing replay entry points.
    pub fn active_claims_for_supersession(
        &self,
        workspace: &WorkspaceId,
        config: &WorkspaceConfig,
    ) -> Result<SupersessionContext, JournalError> {
        let state = self.replay_workspace(workspace, config)?.state;

        let mut claims: Vec<(ClaimId, ClaimView)> = state
            .claims
            .values()
            .filter(|c| c.status == ClaimStatus::Current)
            .map(|c| (c.claim_id.clone(), c.clone()))
            .collect();
        // Deterministic candidate ordering independent of HashMap iteration order.
        claims.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));

        let existing_edges: HashSet<(String, String)> = state
            .superseded
            .values()
            .map(|s| (s.old_claim_id.to_string(), s.new_claim_id.to_string()))
            .collect();

        Ok((claims, existing_edges))
    }
}

/// Ingest markdown sources under `input` into the journal.
pub fn ingest_sources(
    handle: &StoreHandle,
    config: &WorkspaceConfig,
    workspace: &WorkspaceId,
    input: &Path,
    mode: IngestMode,
    observed_at_ms: u64,
    root: &Path,
) -> Result<IngestCommitted, JournalError> {
    let existing = handle.existing_source_hashes(workspace)?;
    let (historical_claims, existing_edges) =
        handle.active_claims_for_supersession(workspace, config)?;
    let plan = crate::ingest::plan_sources_for_config(
        input,
        config,
        workspace,
        observed_at_ms,
        &existing,
        root,
        &historical_claims,
        &existing_edges,
    )?;
    if matches!(mode, IngestMode::DryRun) {
        return Ok(IngestCommitted {
            sources_observed: plan.sources_observed,
            claims_recorded: plan.claims_recorded,
            workspace_id: plan.workspace_id,
            receipts: Vec::new(),
        });
    }

    let mut receipts = Vec::new();
    for action in plan.actions {
        let receipt = match action {
            PlannedAction::Source(payload) => handle.append_source(workspace, &payload)?,
            PlannedAction::Claim(payload) => handle.append_claim(&payload)?,
            PlannedAction::Supersede(payload) => handle.append_superseded(&payload)?,
        };
        receipts.push(receipt);
    }

    Ok(IngestCommitted {
        sources_observed: plan.sources_observed,
        claims_recorded: plan.claims_recorded,
        workspace_id: plan.workspace_id,
        receipts,
    })
}

/// Plan-only ingest for dry runs.
///
/// Loads the workspace's currently-active claims and already-recorded supersession edges
/// from the journal (via [`StoreHandle::active_claims_for_supersession`]) so that the plan
/// reports cross-session supersessions, matching [`ingest_sources`] in [`IngestMode::DryRun`].
pub fn plan_ingest_sources(
    handle: &StoreHandle,
    input: &Path,
    config: &WorkspaceConfig,
    workspace: &WorkspaceId,
    observed_at_ms: u64,
    existing_hashes: &HashSet<String>,
    root: &Path,
) -> Result<IngestPlan, JournalError> {
    let (historical_claims, existing_edges) =
        handle.active_claims_for_supersession(workspace, config)?;
    crate::ingest::plan_sources_for_config(
        input,
        config,
        workspace,
        observed_at_ms,
        existing_hashes,
        root,
        &historical_claims,
        &existing_edges,
    )
    .map(|plan| IngestPlan {
        sources_observed: plan.sources_observed,
        claims_recorded: plan.claims_recorded,
        workspace_id: plan.workspace_id,
    })
}
