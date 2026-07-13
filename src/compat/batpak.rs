//! Small typed boundary over BatPak lifecycle and batch APIs.
//!
//! Texo keeps this module intentionally free of domain policy. BatPak 0.10's
//! store-to-store importer loses its durable idempotency index after a reopen,
//! while its import-provenance constructor is not public. This boundary uses
//! the public raw-page and atomic-batch primitives so the caller can journal a
//! durable materialization ledger in the same commit as copied events.

use std::collections::BTreeSet;
use std::path::Path;

use batpak::coordinate::{Coordinate, Region};
use batpak::event::EventKind;
use batpak::id::{CorrelationId, EntityIdType, IdempotencyKey};
use batpak::store::{
    AppendOptions, BatchAppendItem, CausationRef, ForkOptions, ForkReport, Open, Store, StoreError,
    StoreState,
};
use serde::{Deserialize, Serialize};

/// Maximum source rows in one atomic materialization batch.
pub const IMPORT_PAGE_SIZE: usize = 128;
/// Maximum aggregate payload bytes materialized in one page, except that one
/// valid single event may reach `BatPak`'s 16 MiB default append ceiling.
pub const IMPORT_PAGE_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;

/// Stable source identity carried by the durable caller-owned ledger.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceEventRef {
    /// Source event id as lowercase hex.
    pub event_id_hex: String,
    /// Source global sequence.
    pub global_sequence: u64,
    /// Source event kind raw bits.
    pub kind_raw: u16,
    /// Source payload content hash.
    pub content_hash: [u8; 32],
}

/// One raw source event prepared for destination-local materialization.
pub struct ImportEvent {
    /// Stable source evidence.
    pub source: SourceEventRef,
    coordinate: Coordinate,
    kind: EventKind,
    payload: Vec<u8>,
    correlation_id: CorrelationId,
}

/// Explicit transport form of one importable source event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteImportEvent {
    /// Stable source evidence.
    pub source: SourceEventRef,
    /// Source entity coordinate.
    pub entity: String,
    /// Source scope coordinate.
    pub scope: String,
    /// Raw custom event-kind bits.
    pub kind_raw: u16,
    /// Exact committed payload bytes.
    pub payload: Vec<u8>,
    /// Opaque correlation id.
    pub correlation_id: u128,
}

impl ImportEvent {
    /// Convert a locally read event into the explicit transport schema.
    #[must_use]
    pub fn into_remote(self) -> RemoteImportEvent {
        RemoteImportEvent {
            source: self.source,
            entity: self.coordinate.entity().to_string(),
            scope: self.coordinate.scope().to_string(),
            kind_raw: self.kind.as_raw_u16(),
            payload: self.payload,
            correlation_id: self.correlation_id.as_u128(),
        }
    }

    /// Validate and reconstruct an import event received from a remote source.
    ///
    /// # Errors
    /// Returns configuration failures for malformed coordinates, event kinds,
    /// source ids, or payload content hashes.
    pub fn from_remote(remote: RemoteImportEvent) -> Result<Self, StoreError> {
        let category = u8::try_from(remote.kind_raw >> 12).map_err(|error| {
            StoreError::Configuration(format!("remote event category: {error}"))
        })?;
        let kind = EventKind::try_custom(category, remote.kind_raw & 0x0fff)
            .map_err(|error| StoreError::Configuration(format!("remote event kind: {error:?}")))?;
        if kind.as_raw_u16() != remote.kind_raw {
            return Err(StoreError::Configuration(
                "remote event kind did not round-trip".to_string(),
            ));
        }
        if remote.source.kind_raw != remote.kind_raw
            || !valid_event_id_hex(&remote.source.event_id_hex)
            || batpak::event::hash::compute_hash(&remote.payload) != remote.source.content_hash
        {
            return Err(StoreError::Configuration(
                "remote event source evidence mismatch".to_string(),
            ));
        }
        Ok(Self {
            source: remote.source,
            coordinate: Coordinate::new(remote.entity, remote.scope)
                .map_err(|error| StoreError::Configuration(error.to_string()))?,
            kind,
            payload: remote.payload,
            correlation_id: CorrelationId::from_u128(remote.correlation_id),
        })
    }
}

/// Bounded source page with explicit coverage counters.
pub struct ImportPage {
    /// Events not yet represented in the destination ledger.
    pub events: Vec<ImportEvent>,
    /// Highest source sequence observed, including skipped rows.
    pub high_watermark: Option<u64>,
    /// Source events already represented by the durable ledger.
    pub deduplicated: u64,
    /// Substrate-reserved events intentionally excluded.
    pub skipped_reserved: u64,
    /// Caller-excluded operational event kinds.
    pub skipped_operational: u64,
    /// Whether more rows exist below the captured source ceiling.
    pub has_more: bool,
}

/// Point-in-time identity-preserving fork with deterministic evidence.
///
/// # Errors
/// Returns any lifecycle error surfaced by `BatPak`.
pub fn exact_fork(source: &Store<Open>, destination: &Path) -> Result<ForkReport, StoreError> {
    source.fork_with_evidence(destination, ForkOptions::default())
}

/// Read one bounded page through a call-time source ceiling.
///
/// # Errors
/// Returns any raw source-read failure surfaced by `BatPak`.
pub fn read_import_page<S: StoreState>(
    source: &Store<S>,
    after_global_sequence: Option<u64>,
    source_ceiling: u64,
    represented: &BTreeSet<String>,
    excluded_kind: EventKind,
) -> Result<ImportPage, StoreError> {
    let rows = source.query_entries_after(&Region::all(), after_global_sequence, IMPORT_PAGE_SIZE);
    let mut page = ImportPage {
        events: Vec::new(),
        high_watermark: after_global_sequence,
        deduplicated: 0,
        skipped_reserved: 0,
        skipped_operational: 0,
        has_more: false,
    };
    let mut payload_bytes = 0_usize;
    let mut hit_payload_budget = false;
    for entry in rows {
        if entry.global_sequence() > source_ceiling {
            break;
        }
        let event_id_hex = format!("{:032x}", entry.event_id().as_u128());
        if entry.event_kind().is_reserved() {
            page.high_watermark = Some(entry.global_sequence());
            page.skipped_reserved = page.skipped_reserved.saturating_add(1);
            continue;
        }
        if entry.event_kind() == excluded_kind {
            page.high_watermark = Some(entry.global_sequence());
            page.skipped_operational = page.skipped_operational.saturating_add(1);
            continue;
        }
        if represented.contains(&event_id_hex) {
            page.high_watermark = Some(entry.global_sequence());
            page.deduplicated = page.deduplicated.saturating_add(1);
            continue;
        }
        let raw = source.read_raw(entry.event_id())?;
        if !page.events.is_empty()
            && payload_bytes.saturating_add(raw.event.payload.len()) > IMPORT_PAGE_PAYLOAD_BYTES
        {
            hit_payload_budget = true;
            break;
        }
        page.high_watermark = Some(entry.global_sequence());
        payload_bytes = payload_bytes.saturating_add(raw.event.payload.len());
        page.events.push(ImportEvent {
            source: SourceEventRef {
                event_id_hex,
                global_sequence: entry.global_sequence(),
                kind_raw: entry.event_kind().as_raw_u16(),
                content_hash: raw.event.header.content_hash,
            },
            coordinate: raw.coordinate,
            kind: raw.event.header.event_kind,
            payload: raw.event.payload,
            correlation_id: raw.event.header.correlation_id,
        });
    }
    page.has_more = hit_payload_budget
        || page
            .high_watermark
            .is_some_and(|seen| seen < source_ceiling);
    Ok(page)
}

fn valid_event_id_hex(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Atomically append raw copied events and one caller-built ledger event.
///
/// # Errors
/// Returns append, extension-encoding, or batch-validation errors from `BatPak`.
pub fn append_with_ledger(
    destination: &Store<Open>,
    source_namespace: &str,
    events: Vec<ImportEvent>,
    ledger: BatchAppendItem,
) -> Result<usize, StoreError> {
    let mut items = Vec::with_capacity(events.len().saturating_add(1));
    for event in events {
        let source_id = event.source.event_id_hex.clone();
        let extension = batpak::encoding::to_bytes(&event.source)
            .map_err(|error| StoreError::Serialization(Box::new(error)))?;
        let extension_key = batpak::store::ExtensionKey::new("texo.replica")
            .map_err(|error| StoreError::Configuration(error.to_string()))?;
        let options = AppendOptions::new()
            .with_idempotency(IdempotencyKey::for_operation(
                "texo.replica.import.v1",
                &[source_namespace, &source_id],
            ))
            .with_correlation(event.correlation_id)
            .with_extension(extension_key, extension);
        items.push(BatchAppendItem::from_msgpack_bytes(
            event.coordinate,
            event.kind,
            event.payload,
            options,
            CausationRef::None,
        ));
    }
    let imported = items.len();
    items.push(ledger);
    let _receipts = destination.append_batch(items)?;
    Ok(imported)
}

/// Return the event id at one exact source global sequence.
#[must_use]
pub fn event_id_at<S: StoreState>(source: &Store<S>, sequence: u64) -> Option<String> {
    let after = sequence.checked_sub(1);
    source
        .query_entries_after(&Region::all(), after, 2)
        .into_iter()
        .find(|entry| entry.global_sequence() == sequence)
        .map(|entry| format!("{:032x}", entry.event_id().as_u128()))
}

/// Verify the complete visible hash chain and return the checked event count.
///
/// # Errors
/// Returns a store read error or a corruption-class verification error.
pub fn verify_intact<S: StoreState>(store: &Store<S>) -> Result<usize, StoreError> {
    let report = store.verify_chain()?;
    if report.is_intact() {
        Ok(report.events_checked)
    } else {
        Err(StoreError::Configuration(format!(
            "replica chain is not intact: {} content mismatches, {} dangling links",
            report.content_hash_mismatches.len(),
            report.dangling_links.len()
        )))
    }
}
