//! Deterministic workspace view assembly.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::path::{Path, PathBuf};

use batpak::coordinate::Region;
use batpak::event::{Event, EventKind, EventPayload, EventSourced};
use batpak::id::EntityIdType;
use batpak::store::{Open, Store};
use serde::{Deserialize, Serialize};

use crate::claims::card::ClaimCard;
use crate::claims::conflict::ConflictCard;
use crate::claims::source::SourceCard;
use crate::claims::status::{claim_status, ClaimStatus};
use crate::error::TexoError;
use crate::events::coordinate::scope_for_workspace;
use crate::events::payloads::{
    ClaimRecordedV2, ClaimSupersededV2, ConflictOpenedV2, ConflictResolvedV2, SourceObservedV2,
};

const PAGE_LIMIT: usize = 256;
const SIDECAR_VERSION: u32 = 2;

/// Explicit state of the disposable workspace projection.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionFreshness {
    /// The cached view matches the validated store frontier.
    Fresh,
    /// The store has advanced beyond the cached frontier.
    Stale,
    /// A full fail-closed rebuild is in progress.
    Rebuilding,
    /// The persisted anchor, version, or generations were invalid.
    #[default]
    Invalid,
}

/// Deterministic counters proving whether delta advancement is exercised.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCacheCounters {
    /// Calls to [`assemble`].
    pub assemble_calls: u64,
    /// Entries paged during entity discovery.
    pub discovery_entries_paged: u64,
    /// Events folded directly into cached card states (the delta path).
    pub folded_events: u64,
    /// Per-entity store projection calls (repair path only; 0 in steady state).
    pub project_calls: u64,
    /// Derived workspace-view rebuilds.
    pub view_rebuilds: u64,
    /// Zero-delta returns of the persisted warm view.
    pub warm_view_hits: u64,
}

/// Projection cache for deterministic workspace assembly.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct WorkspaceCache {
    cards: BTreeMap<String, (u64, CachedCard)>,
    frontier: u64,
    anchor_event_id_hex: String,
    view: Option<Arc<WorkspaceView>>,
    freshness: ProjectionFreshness,
    /// Number of projection rebuilds performed by this cache.
    pub project_misses: u64,
    /// Runtime proof counters.
    pub counters: WorkspaceCacheCounters,
    /// True when persisted state (cards/frontier/anchor) changed since the
    /// last load or save; unchanged caches skip the sidecar rewrite entirely.
    dirty: bool,
}

/// Persisted sidecar (v2): folded card states only. The derived view and any
/// discovery bookkeeping are rebuilt on load — persisting them doubled every
/// claim text and made the sidecar outweigh the journal it derives from.
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct PersistedWorkspace {
    version: u32,
    workspace_id: String,
    frontier: u64,
    anchor_event_id_hex: String,
    cache: BTreeMap<String, (u64, CachedCard)>,
}

/// Borrowing twin of [`PersistedWorkspace`] so `save` serializes in place
/// instead of cloning every cached card (the clone doubled peak RSS at scale).
/// Field names and order must match `PersistedWorkspace` exactly.
#[derive(Serialize)]
struct PersistedWorkspaceRef<'a> {
    version: u32,
    workspace_id: &'a str,
    frontier: u64,
    anchor_event_id_hex: &'a str,
    cache: &'a BTreeMap<String, (u64, CachedCard)>,
}

// The anchor detects truncation, forks that replace the frontier event, and
// store swaps. It intentionally does not claim to detect an in-place rewrite
// that preserves both sequence and event id; `Store::verify_chain` remains the
// integrity proof for that threat model.

impl WorkspaceCache {
    /// Load a disposable sidecar. Any read/decode/version mismatch fails closed
    /// to an invalid cache; the next assemble performs a source-truth rebuild.
    #[must_use]
    pub fn load(root: &Path, workspace_id: &str) -> Self {
        let path = sidecar_path(root, workspace_id);
        let Ok(bytes) = std::fs::read(&path) else {
            return Self::default();
        };
        let Ok(persisted) = batpak::encoding::from_bytes::<PersistedWorkspace>(&bytes) else {
            return Self {
                freshness: ProjectionFreshness::Invalid,
                ..Self::default()
            };
        };
        if persisted.version != SIDECAR_VERSION || persisted.workspace_id != workspace_id {
            return Self {
                freshness: ProjectionFreshness::Invalid,
                ..Self::default()
            };
        }
        Self {
            cards: persisted.cache,
            frontier: persisted.frontier,
            anchor_event_id_hex: persisted.anchor_event_id_hex,
            view: None,
            freshness: ProjectionFreshness::Stale,
            project_misses: 0,
            counters: WorkspaceCacheCounters::default(),
            dirty: false,
        }
    }

    /// Whether persisted state changed since the last load or save.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark persisted state as flushed.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Persist the disposable cache with atomic replacement.
    ///
    /// # Errors
    /// Returns I/O or encoding failures; callers may warn and continue because
    /// the journal remains the sole authority.
    pub fn save(&self, root: &Path, workspace_id: &str) -> Result<(), TexoError> {
        let path = sidecar_path(root, workspace_id);
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        std::fs::create_dir_all(parent)?;
        let persisted = PersistedWorkspaceRef {
            version: SIDECAR_VERSION,
            workspace_id,
            frontier: self.frontier,
            anchor_event_id_hex: &self.anchor_event_id_hex,
            cache: &self.cards,
        };
        let bytes = batpak::encoding::to_bytes(&persisted).map_err(|error| TexoError::Decode {
            entity: format!("workspace-cache:{workspace_id}"),
            detail: error.to_string(),
        })?;
        let temporary = path.with_extension("bin.tmp");
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }

    /// Current explicit freshness state.
    #[must_use]
    pub const fn freshness(&self) -> ProjectionFreshness {
        self.freshness
    }
}

fn sidecar_path(root: &Path, workspace_id: &str) -> PathBuf {
    root.join(".texo")
        .join("cache")
        .join("workspace-view")
        .join(format!("{workspace_id}.bin"))
}

/// Cached card by entity kind. Cards are `Arc`-shared with assembled views so
/// a view rebuild bumps refcounts instead of deep-cloning every text field;
/// the fold path takes `Arc::make_mut`, so copy-on-write cost is delta-sized.
/// serde's `rc` feature serializes `Arc<T>` exactly as `T` — sidecar bytes
/// are unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CachedCard {
    /// Cached claim card.
    Claim(Arc<ClaimCard>),
    /// Cached conflict card.
    Conflict(Arc<ConflictCard>),
    /// Cached source card.
    Source(Arc<SourceCard>),
}

/// Assembled deterministic workspace view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceView {
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// Maximum global sequence observed under this workspace scope.
    pub frontier: u64,
    /// Validated projection freshness at this frontier.
    pub freshness: ProjectionFreshness,
    /// Claim views sorted by claim id.
    pub claims: Vec<ClaimView>,
    /// Conflict cards sorted by conflict id.
    pub conflicts: Vec<Arc<ConflictCard>>,
    /// Source cards sorted by source id.
    pub sources: Vec<Arc<SourceCard>>,
}

/// Claim card plus derived relationships.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimView {
    /// Projected claim card (shared with the projection cache).
    pub card: Arc<ClaimCard>,
    /// Derived claim status.
    pub status: ClaimStatus,
    /// Claim ids this claim supersedes, sorted ascending.
    pub supersedes: Vec<String>,
}

/// Assemble a deterministic workspace view from per-entity projections.
///
/// Algorithm:
/// 1. Page visible entries in `workspace:{workspace_id}` with
///    `query_entries_after`, collecting distinct entities and the max global
///    sequence frontier.
/// 2. Bucket entities by `claim:`, `conflict:`, and `source:` prefixes, sorting
///    each bucket before projection.
/// 3. For cached entities, compare `entity_generation` and call
///    `project_if_changed` only when the generation moved; replace cache entries
///    on misses.
/// 4. Derive open-conflict membership and supersession inversions from sorted
///    projected vectors.
///
/// # Errors
/// Returns [`TexoError::Store`] when `BatPak` projection or replay fails.
pub fn assemble(
    store: &Store<Open>,
    workspace_id: &str,
    cache: &mut WorkspaceCache,
) -> Result<Arc<WorkspaceView>, TexoError> {
    cache.counters.assemble_calls = cache.counters.assemble_calls.saturating_add(1);
    let scope = scope_for_workspace(workspace_id);
    let region = Region::scope(&scope);
    if cache.frontier > 0 && !anchor_matches(store, &region, cache) {
        cache.freshness = ProjectionFreshness::Invalid;
    }
    if cache.freshness == ProjectionFreshness::Invalid {
        rebuild_cache(store, &region, cache)?;
    } else {
        advance_cache(store, &region, cache)?;
    }
    if cache.freshness == ProjectionFreshness::Fresh {
        if let Some(view) = &cache.view {
            cache.counters.warm_view_hits = cache.counters.warm_view_hits.saturating_add(1);
            trace_assemble(cache);
            return Ok(Arc::clone(view));
        }
    }

    // One pass over the card map instead of three full walks. The map is
    // keyed by entity string, so each family arrives already id-ordered; the
    // sorts below are near-no-ops kept as the explicit determinism proof.
    let mut claims = Vec::new();
    let mut conflicts = Vec::new();
    let mut sources = Vec::new();
    for (_, card) in cache.cards.values() {
        match card {
            CachedCard::Claim(card) => claims.push(Arc::clone(card)),
            CachedCard::Conflict(card) => conflicts.push(Arc::clone(card)),
            CachedCard::Source(card) => sources.push(Arc::clone(card)),
        }
    }
    claims.sort_by(|left, right| left.claim_id.cmp(&right.claim_id));
    conflicts.sort_by(|left, right| left.conflict_id.cmp(&right.conflict_id));
    sources.sort_by(|left, right| left.source_id.cmp(&right.source_id));

    let mut open_conflict_claims = BTreeSet::new();
    for conflict in &conflicts {
        if conflict.phase == 1 {
            open_conflict_claims.insert(conflict.claim_a.clone());
            open_conflict_claims.insert(conflict.claim_b.clone());
        }
    }

    let mut supersedes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for claim in &claims {
        if let Some(new_id) = &claim.superseded_by {
            supersedes
                .entry(new_id.clone())
                .or_default()
                .push(claim.claim_id.clone());
        }
    }
    for old_ids in supersedes.values_mut() {
        old_ids.sort();
    }

    let claim_views = claims
        .into_iter()
        .map(|card| {
            let in_open_conflict = open_conflict_claims.contains(&card.claim_id);
            let status = claim_status(&card, in_open_conflict);
            let supersedes = supersedes.remove(&card.claim_id).unwrap_or_default();
            ClaimView {
                card,
                status,
                supersedes,
            }
        })
        .collect();

    let view = Arc::new(WorkspaceView {
        workspace_id: workspace_id.to_string(),
        frontier: cache.frontier,
        freshness: ProjectionFreshness::Fresh,
        claims: claim_views,
        conflicts,
        sources,
    });
    cache.view = Some(Arc::clone(&view));
    cache.freshness = ProjectionFreshness::Fresh;
    cache.counters.view_rebuilds = cache.counters.view_rebuilds.saturating_add(1);
    trace_assemble(cache);
    Ok(view)
}

fn trace_assemble(cache: &WorkspaceCache) {
    tracing::debug!(
        frontier = cache.frontier,
        freshness = ?cache.freshness,
        assemble_calls = cache.counters.assemble_calls,
        discovery_entries_paged = cache.counters.discovery_entries_paged,
        project_calls = cache.counters.project_calls,
        view_rebuilds = cache.counters.view_rebuilds,
        warm_view_hits = cache.counters.warm_view_hits,
        "workspace projection assembled"
    );
}

fn anchor_matches(store: &Store<Open>, region: &Region, cache: &WorkspaceCache) -> bool {
    let after = cache.frontier.saturating_sub(1);
    store
        .query_entries_after(region, Some(after), 1)
        .first()
        .is_some_and(|entry| {
            entry.global_sequence() == cache.frontier
                && format!("{:032x}", entry.event_id().as_u128()) == cache.anchor_event_id_hex
        })
}

fn rebuild_cache(
    store: &Store<Open>,
    region: &Region,
    cache: &mut WorkspaceCache,
) -> Result<(), TexoError> {
    cache.freshness = ProjectionFreshness::Rebuilding;
    cache.cards.clear();
    cache.frontier = 0;
    cache.anchor_event_id_hex.clear();
    cache.view = None;
    fold_after(store, region, None, cache)?;
    cache.dirty = true;
    cache.freshness = ProjectionFreshness::Stale;
    Ok(())
}

fn advance_cache(
    store: &Store<Open>,
    region: &Region,
    cache: &mut WorkspaceCache,
) -> Result<(), TexoError> {
    match fold_after(store, region, Some(cache.frontier), cache)? {
        FoldOutcome::NoDelta => {
            cache.freshness = ProjectionFreshness::Fresh;
            Ok(())
        }
        FoldOutcome::Folded => {
            cache.dirty = true;
            cache.freshness = ProjectionFreshness::Stale;
            cache.view = None;
            Ok(())
        }
        FoldOutcome::GenerationRegressed => {
            cache.freshness = ProjectionFreshness::Invalid;
            rebuild_cache(store, region, cache)
        }
    }
}

enum FoldOutcome {
    NoDelta,
    Folded,
    GenerationRegressed,
}

/// Page every entry after `initial_after` and fold each event directly into
/// its cached card state. One pass, O(delta): the paged entries already carry
/// everything the fold needs, so no per-entity store replays occur (each
/// `Store::project` call rebuilds an O(region) replay plan — the quadratic
/// knee this module previously bent at).
fn fold_after(
    store: &Store<Open>,
    region: &Region,
    initial_after: Option<u64>,
    cache: &mut WorkspaceCache,
) -> Result<FoldOutcome, TexoError> {
    let mut after = initial_after;
    // Generation each dirty entity had before this fold; used for the
    // fail-closed regression check once authoritative generations are known.
    let mut prior_generations: BTreeMap<String, u64> = BTreeMap::new();
    loop {
        let page = store.query_entries_after(region, after, PAGE_LIMIT);
        if page.is_empty() {
            break;
        }
        cache.counters.discovery_entries_paged = cache
            .counters
            .discovery_entries_paged
            .saturating_add(u64::try_from(page.len()).unwrap_or(u64::MAX));
        for entry in &page {
            let entity = entry.coord().entity();
            let Some(family) = CardFamily::of(entity) else {
                continue; // relation:*, session lanes, workspace-meta: not card state
            };
            if !prior_generations.contains_key(entity) {
                let prior = cache.cards.get(entity).map_or(0, |(g, _)| *g);
                prior_generations.insert(entity.to_string(), prior);
            }
            let raw = store.read_raw(entry.event_id())?;
            fold_event(cache, entity, family, entry.event_kind(), &raw.event);
        }
        if let Some(last) = page.last() {
            // Frontier and anchor are last-writer-in-paging-order state: one
            // assignment per page, not one heap-allocating format per entry.
            cache.frontier = last.global_sequence();
            cache.anchor_event_id_hex = format!("{:032x}", last.event_id().as_u128());
        }
        after = page.last().map(batpak::store::IndexEntry::global_sequence);
    }
    if prior_generations.is_empty() {
        return Ok(FoldOutcome::NoDelta);
    }
    // Stamp authoritative generations; a store generation below the pre-fold
    // value means the entity stream went backwards under us — fail closed.
    for (entity, prior) in &prior_generations {
        let generation = store.entity_generation(entity).unwrap_or(0);
        if generation < *prior {
            return Ok(FoldOutcome::GenerationRegressed);
        }
        if let Some(slot) = cache.cards.get_mut(entity) {
            slot.0 = generation;
        }
    }
    Ok(FoldOutcome::Folded)
}

/// Card family an entity belongs to, by coordinate prefix.
#[derive(Clone, Copy)]
enum CardFamily {
    Claim,
    Conflict,
    Source,
}

impl CardFamily {
    fn of(entity: &str) -> Option<Self> {
        if entity.starts_with("claim:") {
            Some(Self::Claim)
        } else if entity.starts_with("conflict:") {
            Some(Self::Conflict)
        } else if entity.starts_with("source:") {
            Some(Self::Source)
        } else {
            None
        }
    }
}

/// Fold one raw event into the cached card for `entity`.
///
/// Kinds outside the family's registered set are skipped explicitly (belt) in
/// addition to the derive's own unknown-kind no-op (suspenders), so a future
/// event kind landing on a card entity cannot corrupt folded state.
fn fold_event(
    cache: &mut WorkspaceCache,
    entity: &str,
    family: CardFamily,
    kind: EventKind,
    event: &Event<Vec<u8>>,
) {
    let relevant = match family {
        CardFamily::Claim => {
            kind == <ClaimRecordedV2 as EventPayload>::KIND
                || kind == <ClaimSupersededV2 as EventPayload>::KIND
        }
        CardFamily::Conflict => {
            kind == <ConflictOpenedV2 as EventPayload>::KIND
                || kind == <ConflictResolvedV2 as EventPayload>::KIND
        }
        CardFamily::Source => kind == <SourceObservedV2 as EventPayload>::KIND,
    };
    if !relevant {
        return;
    }
    if !cache.cards.contains_key(entity) {
        cache.cards.insert(
            entity.to_string(),
            match family {
                CardFamily::Claim => (0, CachedCard::Claim(Arc::new(ClaimCard::default()))),
                CardFamily::Conflict => (0, CachedCard::Conflict(Arc::new(ConflictCard::default()))),
                CardFamily::Source => (0, CachedCard::Source(Arc::new(SourceCard::default()))),
            },
        );
    }
    let Some(slot) = cache.cards.get_mut(entity) else {
        return;
    };
    match (&mut slot.1, family) {
        (CachedCard::Claim(card), CardFamily::Claim) => Arc::make_mut(card).apply_event(event),
        (CachedCard::Conflict(card), CardFamily::Conflict) => {
            Arc::make_mut(card).apply_event(event);
        }
        (CachedCard::Source(card), CardFamily::Source) => Arc::make_mut(card).apply_event(event),
        // A prefix can only ever map to one family; a mismatch means the
        // cached slot predates a (nonexistent) entity-family change.
        _ => debug_assert!(false, "cached card family mismatch for {entity}"),
    }
    cache.counters.folded_events = cache.counters.folded_events.saturating_add(1);
}
