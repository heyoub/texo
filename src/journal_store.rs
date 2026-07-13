//! Authority-preserving wrapper over BatPak's writable and read-only states.

use std::sync::Arc;

use batpak::coordinate::Region;
use batpak::event::{EventPayload, EventSourced, StoredEvent};
use batpak::store::{
    AppendOptions, AppendReceipt, ChainVerificationReport, Freshness, FrontierView, IndexEntry,
    Open, ReadOnly, ReceiptVerification, ReplayInput, Store, StoreError, StoreState, StoreStats,
};

/// Read contract shared by canonical and replica journal handles.
pub trait JournalRead {
    /// Query one bounded page after an exclusive sequence.
    fn query_entries_after(
        &self,
        region: &Region,
        after: Option<u64>,
        limit: usize,
    ) -> Vec<IndexEntry>;

    /// Read one raw stored event.
    ///
    /// # Errors
    /// Returns a store error when the event is absent or unreadable.
    fn read_raw(&self, event_id: batpak::id::EventId) -> Result<StoredEvent<Vec<u8>>, StoreError>;

    /// Return the entity generation used by incremental projections.
    fn entity_generation(&self, entity: &str) -> Option<u64>;
}

impl<State: StoreState> JournalRead for Store<State> {
    fn query_entries_after(
        &self,
        region: &Region,
        after: Option<u64>,
        limit: usize,
    ) -> Vec<IndexEntry> {
        Store::query_entries_after(self, region, after, limit)
    }

    fn read_raw(&self, event_id: batpak::id::EventId) -> Result<StoredEvent<Vec<u8>>, StoreError> {
        Store::read_raw(self, event_id)
    }

    fn entity_generation(&self, entity: &str) -> Option<u64> {
        Store::entity_generation(self, entity)
    }
}

impl JournalRead for JournalStore {
    fn query_entries_after(
        &self,
        region: &Region,
        after: Option<u64>,
        limit: usize,
    ) -> Vec<IndexEntry> {
        Self::query_entries_after(self, region, after, limit)
    }

    fn read_raw(&self, event_id: batpak::id::EventId) -> Result<StoredEvent<Vec<u8>>, StoreError> {
        Self::read_raw(self, event_id)
    }

    fn entity_generation(&self, entity: &str) -> Option<u64> {
        Self::entity_generation(self, entity)
    }
}

/// One opened physical journal with its write capability represented by type.
#[derive(Clone)]
pub enum JournalStore {
    /// Canonical journal with an active writer.
    Writable(Arc<Store<Open>>),
    /// Replica journal opened without a writer or lifecycle append.
    ReadOnly(Arc<Store<ReadOnly>>),
}

impl JournalStore {
    /// Wrap a canonical writable store.
    #[must_use]
    pub fn writable(store: Arc<Store<Open>>) -> Self {
        Self::Writable(store)
    }

    /// Wrap a read-only replica store.
    #[must_use]
    pub fn read_only(store: Arc<Store<ReadOnly>>) -> Self {
        Self::ReadOnly(store)
    }

    /// Borrow the canonical writer when this journal owns authority.
    #[must_use]
    pub fn writable_arc(&self) -> Option<Arc<Store<Open>>> {
        match self {
            Self::Writable(store) => Some(Arc::clone(store)),
            Self::ReadOnly(_) => None,
        }
    }

    /// Query one bounded page after an exclusive sequence.
    #[must_use]
    pub fn query_entries_after(
        &self,
        region: &Region,
        after: Option<u64>,
        limit: usize,
    ) -> Vec<IndexEntry> {
        match self {
            Self::Writable(store) => store.query_entries_after(region, after, limit),
            Self::ReadOnly(store) => store.query_entries_after(region, after, limit),
        }
    }

    /// Read one raw stored event.
    ///
    /// # Errors
    /// Returns a store error when the event is absent or unreadable.
    pub fn read_raw(
        &self,
        event_id: batpak::id::EventId,
    ) -> Result<StoredEvent<Vec<u8>>, StoreError> {
        match self {
            Self::Writable(store) => store.read_raw(event_id),
            Self::ReadOnly(store) => store.read_raw(event_id),
        }
    }

    /// Return all visible rows for one entity.
    #[must_use]
    pub fn by_entity(&self, entity: &str) -> Vec<IndexEntry> {
        match self {
            Self::Writable(store) => store.by_entity(entity),
            Self::ReadOnly(store) => store.by_entity(entity),
        }
    }

    /// Return all visible rows for one scope.
    #[must_use]
    pub fn by_scope(&self, scope: &str) -> Vec<IndexEntry> {
        match self {
            Self::Writable(store) => store.by_scope(scope),
            Self::ReadOnly(store) => store.by_scope(scope),
        }
    }

    /// Return one entity lane in sequence order.
    #[must_use]
    pub fn stream_lane(&self, entity: &str, lane: u32) -> Vec<IndexEntry> {
        match self {
            Self::Writable(store) => store.stream_lane(entity, lane),
            Self::ReadOnly(store) => store.stream_lane(entity, lane),
        }
    }

    /// Return the entity generation used by incremental projections.
    #[must_use]
    pub fn entity_generation(&self, entity: &str) -> Option<u64> {
        match self {
            Self::Writable(store) => store.entity_generation(entity),
            Self::ReadOnly(store) => store.entity_generation(entity),
        }
    }

    /// Recompute the complete visible hash chain.
    ///
    /// # Errors
    /// Returns a store error when committed bytes cannot be read or verified.
    pub fn verify_chain(&self) -> Result<ChainVerificationReport, StoreError> {
        match self {
            Self::Writable(store) => store.verify_chain(),
            Self::ReadOnly(store) => store.verify_chain(),
        }
    }

    /// Verify one append receipt against this physical journal.
    #[must_use]
    pub fn verify_append_receipt(&self, receipt: &AppendReceipt) -> ReceiptVerification {
        match self {
            Self::Writable(store) => store.verify_append_receipt(receipt),
            Self::ReadOnly(store) => store.verify_append_receipt(receipt),
        }
    }

    /// Return the current frontier view.
    pub fn frontier(&self) -> FrontierView {
        match self {
            Self::Writable(store) => store.frontier(),
            Self::ReadOnly(store) => store.frontier(),
        }
    }

    /// Return lightweight runtime statistics.
    pub fn stats(&self) -> StoreStats {
        match self {
            Self::Writable(store) => store.stats(),
            Self::ReadOnly(store) => store.stats(),
        }
    }

    /// Reconstruct one typed projection under the requested freshness policy.
    ///
    /// # Errors
    /// Returns replay, decode, cache, or store read failures.
    pub fn project<T>(&self, entity: &str, freshness: &Freshness) -> Result<Option<T>, StoreError>
    where
        T: EventSourced + serde::Serialize + serde::de::DeserializeOwned + 'static,
        T::Input: ReplayInput,
    {
        match self {
            Self::Writable(store) => store.project(entity, freshness),
            Self::ReadOnly(store) => store.project(entity, freshness),
        }
    }

    /// Append a typed payload only when this journal owns write authority.
    ///
    /// # Errors
    /// Returns a configuration error for replicas or an append failure.
    pub fn append_typed<T: EventPayload>(
        &self,
        coordinate: &batpak::coordinate::Coordinate,
        payload: &T,
    ) -> Result<AppendReceipt, StoreError> {
        self.writer()?.append_typed(coordinate, payload)
    }

    /// Append a typed payload with options only on a canonical journal.
    ///
    /// # Errors
    /// Returns a configuration error for replicas or an append failure.
    pub fn append_typed_with_options<T: EventPayload>(
        &self,
        coordinate: &batpak::coordinate::Coordinate,
        payload: &T,
        options: AppendOptions,
    ) -> Result<AppendReceipt, StoreError> {
        self.writer()?
            .append_typed_with_options(coordinate, payload, options)
    }

    fn writer(&self) -> Result<&Store<Open>, StoreError> {
        match self {
            Self::Writable(store) => Ok(store.as_ref()),
            Self::ReadOnly(_) => Err(StoreError::Configuration(
                "read-only journal has no append capability".to_string(),
            )),
        }
    }
}
