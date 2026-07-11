//! Deterministic workspace view assembly.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use batpak::coordinate::Region;
use batpak::id::EntityIdType;
use batpak::store::{Freshness, Open, Store};
use serde::{Deserialize, Serialize};

use crate::claims::card::ClaimCard;
use crate::claims::conflict::ConflictCard;
use crate::claims::source::SourceCard;
use crate::claims::status::{claim_status, ClaimStatus};
use crate::error::TexoError;
use crate::events::coordinate::scope_for_workspace;

const PAGE_LIMIT: usize = 256;
const SIDECAR_VERSION: u32 = 1;

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
    /// Per-entity projection calls.
    pub project_calls: u64,
    /// Derived workspace-view rebuilds.
    pub view_rebuilds: u64,
    /// Zero-delta returns of the persisted warm view.
    pub warm_view_hits: u64,
}

/// Projection cache for deterministic workspace assembly.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceCache {
    cards: BTreeMap<String, (u64, CachedCard)>,
    entities: BTreeSet<String>,
    frontier: u64,
    anchor_event_id_hex: String,
    view: Option<WorkspaceView>,
    freshness: ProjectionFreshness,
    /// Number of projection rebuilds performed by this cache.
    pub project_misses: u64,
    /// Runtime proof counters.
    pub counters: WorkspaceCacheCounters,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PersistedWorkspace {
    version: u32,
    workspace_id: String,
    frontier: u64,
    anchor_event_id_hex: String,
    entities: BTreeSet<String>,
    cache: BTreeMap<String, (u64, CachedCard)>,
    view: Option<WorkspaceView>,
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
            entities: persisted.entities,
            frontier: persisted.frontier,
            anchor_event_id_hex: persisted.anchor_event_id_hex,
            view: persisted.view,
            freshness: ProjectionFreshness::Stale,
            project_misses: 0,
            counters: WorkspaceCacheCounters::default(),
        }
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
        let persisted = PersistedWorkspace {
            version: SIDECAR_VERSION,
            workspace_id: workspace_id.to_string(),
            frontier: self.frontier,
            anchor_event_id_hex: self.anchor_event_id_hex.clone(),
            entities: self.entities.clone(),
            cache: self.cards.clone(),
            view: self.view.clone(),
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

/// Cached card by entity kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CachedCard {
    /// Cached claim card.
    Claim(ClaimCard),
    /// Cached conflict card.
    Conflict(ConflictCard),
    /// Cached source card.
    Source(SourceCard),
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
    pub conflicts: Vec<ConflictCard>,
    /// Source cards sorted by source id.
    pub sources: Vec<SourceCard>,
}

/// Claim card plus derived relationships.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimView {
    /// Projected claim card.
    pub card: ClaimCard,
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
) -> Result<WorkspaceView, TexoError> {
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
            return Ok(view.clone());
        }
    }

    let mut claims = cache
        .cards
        .values()
        .filter_map(|(_, card)| match card {
            CachedCard::Claim(card) => Some(card.clone()),
            CachedCard::Conflict(_) | CachedCard::Source(_) => None,
        })
        .collect::<Vec<_>>();
    claims.sort_by(|left, right| left.claim_id.cmp(&right.claim_id));
    let mut conflicts = cache
        .cards
        .values()
        .filter_map(|(_, card)| match card {
            CachedCard::Conflict(card) => Some(card.clone()),
            CachedCard::Claim(_) | CachedCard::Source(_) => None,
        })
        .collect::<Vec<_>>();
    conflicts.sort_by(|left, right| left.conflict_id.cmp(&right.conflict_id));
    let mut sources = cache
        .cards
        .values()
        .filter_map(|(_, card)| match card {
            CachedCard::Source(card) => Some(card.clone()),
            CachedCard::Claim(_) | CachedCard::Conflict(_) => None,
        })
        .collect::<Vec<_>>();
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

    let view = WorkspaceView {
        workspace_id: workspace_id.to_string(),
        frontier: cache.frontier,
        freshness: ProjectionFreshness::Fresh,
        claims: claim_views,
        conflicts,
        sources,
    };
    cache.view = Some(view.clone());
    cache.freshness = ProjectionFreshness::Fresh;
    cache.counters.view_rebuilds = cache.counters.view_rebuilds.saturating_add(1);
    Ok(view)
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
    cache.entities.clear();
    cache.frontier = 0;
    cache.anchor_event_id_hex.clear();
    cache.view = None;
    let dirty = discover_after(store, region, None, cache);
    project_dirty(store, &dirty, cache)?;
    cache.freshness = ProjectionFreshness::Stale;
    Ok(())
}

fn advance_cache(
    store: &Store<Open>,
    region: &Region,
    cache: &mut WorkspaceCache,
) -> Result<(), TexoError> {
    let dirty = discover_after(store, region, Some(cache.frontier), cache);
    if dirty.is_empty() {
        cache.freshness = ProjectionFreshness::Fresh;
        return Ok(());
    }
    cache.freshness = ProjectionFreshness::Stale;
    for entity in &dirty {
        if let Some((cached_generation, _)) = cache.cards.get(entity) {
            if store.entity_generation(entity).unwrap_or(0) < *cached_generation {
                cache.freshness = ProjectionFreshness::Invalid;
                return rebuild_cache(store, region, cache);
            }
        }
    }
    project_dirty(store, &dirty, cache)
}

fn discover_after(
    store: &Store<Open>,
    region: &Region,
    initial_after: Option<u64>,
    cache: &mut WorkspaceCache,
) -> BTreeSet<String> {
    let mut after = initial_after;
    let mut dirty = BTreeSet::new();
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
            let entity = entry.coord().entity().to_string();
            cache.entities.insert(entity.clone());
            dirty.insert(entity);
            cache.frontier = entry.global_sequence();
            cache.anchor_event_id_hex = format!("{:032x}", entry.event_id().as_u128());
        }
        after = page.last().map(batpak::store::IndexEntry::global_sequence);
    }
    dirty
}

fn project_dirty(
    store: &Store<Open>,
    dirty: &BTreeSet<String>,
    cache: &mut WorkspaceCache,
) -> Result<(), TexoError> {
    for entity in dirty {
        if entity.starts_with("claim:") {
            let _ = project_claim(store, entity, cache)?;
        } else if entity.starts_with("conflict:") {
            let _ = project_conflict(store, entity, cache)?;
        } else if entity.starts_with("source:") {
            let _ = project_source(store, entity, cache)?;
        }
    }
    cache.view = None;
    Ok(())
}

fn project_claim(
    store: &Store<Open>,
    entity: &str,
    cache: &mut WorkspaceCache,
) -> Result<Option<ClaimCard>, TexoError> {
    let generation = store.entity_generation(entity).unwrap_or(0);
    if let Some((cached_generation, CachedCard::Claim(card))) = cache.cards.get(entity) {
        if *cached_generation == generation {
            return Ok(Some(card.clone()));
        }
        cache.counters.project_calls = cache.counters.project_calls.saturating_add(1);
        if let Some((returned_generation, projected)) = store.project_if_changed::<ClaimCard>(
            entity,
            *cached_generation,
            &Freshness::Consistent,
        )? {
            cache.project_misses = cache.project_misses.saturating_add(1);
            return Ok(projected.inspect(|card| {
                cache.cards.insert(
                    entity.to_string(),
                    (returned_generation, CachedCard::Claim(card.clone())),
                );
            }));
        }
        return Ok(Some(card.clone()));
    }

    cache.project_misses = cache.project_misses.saturating_add(1);
    cache.counters.project_calls = cache.counters.project_calls.saturating_add(1);
    let projected = store.project::<ClaimCard>(entity, &Freshness::Consistent)?;
    if let Some(card) = &projected {
        cache.cards.insert(
            entity.to_string(),
            (generation, CachedCard::Claim(card.clone())),
        );
    }
    Ok(projected)
}

fn project_conflict(
    store: &Store<Open>,
    entity: &str,
    cache: &mut WorkspaceCache,
) -> Result<Option<ConflictCard>, TexoError> {
    let generation = store.entity_generation(entity).unwrap_or(0);
    if let Some((cached_generation, CachedCard::Conflict(card))) = cache.cards.get(entity) {
        if *cached_generation == generation {
            return Ok(Some(card.clone()));
        }
        cache.counters.project_calls = cache.counters.project_calls.saturating_add(1);
        if let Some((returned_generation, projected)) = store.project_if_changed::<ConflictCard>(
            entity,
            *cached_generation,
            &Freshness::Consistent,
        )? {
            cache.project_misses = cache.project_misses.saturating_add(1);
            return Ok(projected.inspect(|card| {
                cache.cards.insert(
                    entity.to_string(),
                    (returned_generation, CachedCard::Conflict(card.clone())),
                );
            }));
        }
        return Ok(Some(card.clone()));
    }

    cache.project_misses = cache.project_misses.saturating_add(1);
    cache.counters.project_calls = cache.counters.project_calls.saturating_add(1);
    let projected = store.project::<ConflictCard>(entity, &Freshness::Consistent)?;
    if let Some(card) = &projected {
        cache.cards.insert(
            entity.to_string(),
            (generation, CachedCard::Conflict(card.clone())),
        );
    }
    Ok(projected)
}

fn project_source(
    store: &Store<Open>,
    entity: &str,
    cache: &mut WorkspaceCache,
) -> Result<Option<SourceCard>, TexoError> {
    let generation = store.entity_generation(entity).unwrap_or(0);
    if let Some((cached_generation, CachedCard::Source(card))) = cache.cards.get(entity) {
        if *cached_generation == generation {
            return Ok(Some(card.clone()));
        }
        cache.counters.project_calls = cache.counters.project_calls.saturating_add(1);
        if let Some((returned_generation, projected)) = store.project_if_changed::<SourceCard>(
            entity,
            *cached_generation,
            &Freshness::Consistent,
        )? {
            cache.project_misses = cache.project_misses.saturating_add(1);
            return Ok(projected.inspect(|card| {
                cache.cards.insert(
                    entity.to_string(),
                    (returned_generation, CachedCard::Source(card.clone())),
                );
            }));
        }
        return Ok(Some(card.clone()));
    }

    cache.project_misses = cache.project_misses.saturating_add(1);
    cache.counters.project_calls = cache.counters.project_calls.saturating_add(1);
    let projected = store.project::<SourceCard>(entity, &Freshness::Consistent)?;
    if let Some(card) = &projected {
        cache.cards.insert(
            entity.to_string(),
            (generation, CachedCard::Source(card.clone())),
        );
    }
    Ok(projected)
}
