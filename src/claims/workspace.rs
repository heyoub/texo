//! Deterministic workspace view assembly.

use std::collections::{BTreeMap, BTreeSet};

use batpak::coordinate::Region;
use batpak::store::{Freshness, Open, Store};
use serde::{Deserialize, Serialize};

use crate::claims::card::ClaimCard;
use crate::claims::conflict::ConflictCard;
use crate::claims::source::SourceCard;
use crate::claims::status::{claim_status, ClaimStatus};
use crate::error::TexoError;
use crate::events::coordinate::scope_for_workspace;

const PAGE_LIMIT: usize = 256;

/// Projection cache for deterministic workspace assembly.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceCache {
    cards: BTreeMap<String, (u64, CachedCard)>,
    /// Number of projection rebuilds performed by this cache.
    pub project_misses: u64,
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
    let scope = scope_for_workspace(workspace_id);
    let region = Region::scope(&scope);
    let mut after = None;
    let mut frontier = 0;
    let mut entities = BTreeSet::new();

    loop {
        let page = store.query_entries_after(&region, after, PAGE_LIMIT);
        if page.is_empty() {
            break;
        }
        for entry in &page {
            frontier = frontier.max(entry.global_sequence());
            entities.insert(entry.coord().entity().to_string());
        }
        after = page.last().map(batpak::store::IndexEntry::global_sequence);
    }

    let mut claim_entities = Vec::new();
    let mut conflict_entities = Vec::new();
    let mut source_entities = Vec::new();

    for entity in entities {
        if entity.starts_with("claim:") {
            claim_entities.push(entity);
        } else if entity.starts_with("conflict:") {
            conflict_entities.push(entity);
        } else if entity.starts_with("source:") {
            source_entities.push(entity);
        }
    }

    let mut claims = Vec::new();
    for entity in &claim_entities {
        if let Some(card) = project_claim(store, entity, cache)? {
            claims.push(card);
        }
    }
    claims.sort_by(|left, right| left.claim_id.cmp(&right.claim_id));

    let mut conflicts = Vec::new();
    for entity in &conflict_entities {
        if let Some(card) = project_conflict(store, entity, cache)? {
            conflicts.push(card);
        }
    }
    conflicts.sort_by(|left, right| left.conflict_id.cmp(&right.conflict_id));

    let mut sources = Vec::new();
    for entity in &source_entities {
        if let Some(card) = project_source(store, entity, cache)? {
            sources.push(card);
        }
    }
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

    Ok(WorkspaceView {
        workspace_id: workspace_id.to_string(),
        frontier,
        claims: claim_views,
        conflicts,
        sources,
    })
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
    let projected = store.project::<SourceCard>(entity, &Freshness::Consistent)?;
    if let Some(card) = &projected {
        cache.cards.insert(
            entity.to_string(),
            (generation, CachedCard::Source(card.clone())),
        );
    }
    Ok(projected)
}
