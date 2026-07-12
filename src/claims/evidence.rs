//! Deterministic replay projection joining exact evidence to semantic claims.

use std::collections::BTreeMap;

use batpak::coordinate::Region;
use batpak::event::EventPayload;
use batpak::store::{Open, Store};

use crate::error::TexoError;
use crate::events::coordinate::scope_for_workspace;
use crate::events::payloads::{
    ClaimEvidenceLinkedV1, EvidenceOccurrenceRecordedV1, EvidenceReconciliationAcceptedV1,
};
use crate::knowledge::{ClaimEvidence, ReconciliationProvenance};

const PAGE_LIMIT: usize = 256;

/// Snapshot-bounded evidence view assembled exclusively from journal events.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EvidenceProjection {
    by_claim: BTreeMap<String, Vec<ClaimEvidence>>,
    incomplete: bool,
}

impl EvidenceProjection {
    /// Remove and return evidence linked to one claim.
    pub fn take_claim(&mut self, claim_id: &str) -> Vec<ClaimEvidence> {
        self.by_claim.remove(claim_id).unwrap_or_default()
    }

    /// Borrow evidence linked to one claim.
    #[must_use]
    pub fn for_claim(&self, claim_id: &str) -> &[ClaimEvidence] {
        self.by_claim.get(claim_id).map_or(&[], Vec::as_slice)
    }

    /// Whether replay found a link whose occurrence was unavailable.
    #[must_use]
    pub const fn is_incomplete(&self) -> bool {
        self.incomplete
    }
}

/// Assemble exact claim evidence through one durable frontier.
///
/// Replay is read-only and performs no Git, parser, indexer, or model calls.
/// Links are joined after the complete bounded scan so event discovery order
/// cannot affect the result.
///
/// # Errors
/// Returns a store or typed-decode error for unreadable journal source truth.
pub fn assemble_through(
    store: &Store<Open>,
    workspace_id: &str,
    frontier: u64,
) -> Result<EvidenceProjection, TexoError> {
    let region = Region::scope(scope_for_workspace(workspace_id));
    let mut after = None;
    let mut occurrences = BTreeMap::new();
    let mut reconciliations = BTreeMap::new();
    let mut links = Vec::new();
    'pages: loop {
        let page = store.query_entries_after(&region, after, PAGE_LIMIT);
        if page.is_empty() {
            break;
        }
        for entry in &page {
            if entry.global_sequence() > frontier {
                break 'pages;
            }
            if entry.event_kind() == <EvidenceOccurrenceRecordedV1 as EventPayload>::KIND {
                let raw = store.read_raw(entry.event_id())?;
                let payload = decode::<EvidenceOccurrenceRecordedV1>(
                    entry.coord().entity(),
                    &raw.event.payload,
                )?;
                occurrences.insert(
                    payload.occurrence.occurrence_id.clone(),
                    (payload.occurrence, entry.global_sequence()),
                );
            } else if entry.event_kind() == <ClaimEvidenceLinkedV1 as EventPayload>::KIND {
                let raw = store.read_raw(entry.event_id())?;
                let payload =
                    decode::<ClaimEvidenceLinkedV1>(entry.coord().entity(), &raw.event.payload)?;
                links.push((payload, entry.global_sequence()));
            } else if entry.event_kind() == <EvidenceReconciliationAcceptedV1 as EventPayload>::KIND
            {
                let raw = store.read_raw(entry.event_id())?;
                let payload = decode::<EvidenceReconciliationAcceptedV1>(
                    entry.coord().entity(),
                    &raw.event.payload,
                )?;
                reconciliations
                    .entry((
                        payload.claim_id.to_string(),
                        payload.occurrence_id.to_string(),
                    ))
                    .or_insert((payload, entry.global_sequence()));
            }
        }
        after = page.last().map(batpak::store::IndexEntry::global_sequence);
    }
    Ok(join(&occurrences, &reconciliations, links))
}

fn decode<T: serde::de::DeserializeOwned>(entity: &str, bytes: &[u8]) -> Result<T, TexoError> {
    batpak::encoding::from_bytes(bytes).map_err(|error| TexoError::Decode {
        entity: entity.to_string(),
        detail: error.to_string(),
    })
}

fn join(
    occurrences: &BTreeMap<
        crate::knowledge::EvidenceOccurrenceId,
        (crate::knowledge::EvidenceOccurrence, u64),
    >,
    reconciliations: &BTreeMap<(String, String), (EvidenceReconciliationAcceptedV1, u64)>,
    links: Vec<(ClaimEvidenceLinkedV1, u64)>,
) -> EvidenceProjection {
    let mut projection = EvidenceProjection::default();
    for (link, link_sequence) in links {
        let Some((occurrence, occurrence_sequence)) = occurrences.get(&link.occurrence_id) else {
            projection.incomplete = true;
            continue;
        };
        projection
            .by_claim
            .entry(link.claim_id.to_string())
            .or_default()
            .push(ClaimEvidence {
                claim_id: link.claim_id.to_string(),
                occurrence: occurrence.clone(),
                stance: link.stance,
                method: link.method,
                occurrence_sequence: *occurrence_sequence,
                link_sequence,
                reconciliation: reconciliations
                    .get(&(link.claim_id.to_string(), link.occurrence_id.to_string()))
                    .map(|(accepted, acceptance_sequence)| ReconciliationProvenance {
                        score_ppm: accepted.score_ppm,
                        judge_fingerprint: accepted.judge_fingerprint.clone(),
                        cache_key_hex: accepted.cache_key_hex.clone(),
                        policy_version: accepted.policy_version.clone(),
                        observed_at_ms: accepted.observed_at_ms,
                        acceptance_sequence: *acceptance_sequence,
                    }),
            });
    }
    for evidence in projection.by_claim.values_mut() {
        evidence.sort_by(|left, right| {
            left.occurrence
                .path
                .cmp(&right.occurrence.path)
                .then_with(|| {
                    left.occurrence
                        .byte_range
                        .start
                        .cmp(&right.occurrence.byte_range.start)
                })
                .then_with(|| left.link_sequence.cmp(&right.link_sequence))
        });
    }
    projection
}
