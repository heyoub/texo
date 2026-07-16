use super::super::common::append_json;
use super::backend::semantic_error;
use super::{RejudgedPairRow, DEFAULT_RELATE_CACHE, ENV_RELATE_CACHE};
use crate::error::TexoError;
use crate::events::coordinate::scope_for_workspace;
use crate::events::ids::{ClaimId, WorkspaceId};
use crate::events::payloads::RelationJudgedV1;
use crate::ops::env;
use crate::semantics::pipeline::ClaimView as SemanticClaimView;
use crate::semantics::score::{ppm_to_unit_interval, unit_interval_to_ppm};
use batpak::coordinate::Region;
use batpak::event::EventPayload;
use batpak::event::EventSourced;
use batpak::store::Freshness;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(in crate::ops::handlers) struct SettlementAuthority {
    pub(in crate::ops::handlers) verdicts:
        BTreeMap<(ClaimId, ClaimId), crate::semantics::RelationVerdict>,
    pub(in crate::ops::handlers) cache_keys: BTreeMap<(String, String), String>,
    pub(in crate::ops::handlers) warnings: Vec<crate::relate::settlement::AuthorityWarning>,
    pub(in crate::ops::handlers) unresolved_pairs: usize,
}

pub(in crate::ops::handlers) fn authoritative_settlements(
    frontier: Option<u64>,
) -> Result<SettlementAuthority, TexoError> {
    env::with(|op_env| {
        let entities = settlement_entities(&op_env.store, &op_env.workspace_id, frontier);

        let mut settled = BTreeMap::new();
        let mut cache_keys = BTreeMap::new();
        let mut warnings = Vec::new();
        let mut unresolved_pairs = 0;
        for entity in entities {
            let card = if let Some(frontier) = frontier {
                let mut card = crate::claims::settlement::SettlementCard::default();
                for entry in op_env.store.by_entity(&entity) {
                    if entry.global_sequence() > frontier {
                        break;
                    }
                    let raw = op_env.store.read_raw(entry.event_id())?;
                    card.apply_event(&raw.event);
                }
                card
            } else {
                let Some(card) = env::deterministic_projection(|| {
                    op_env
                        .store
                        .project::<crate::claims::settlement::SettlementCard>(
                            &entity,
                            &Freshness::Consistent,
                        )
                })?
                else {
                    continue;
                };
                card
            };
            let Some(authoritative) = card.authoritative.as_ref() else {
                if !card.deferrals.is_empty() {
                    unresolved_pairs += 1;
                }
                continue;
            };
            let older = ClaimId::try_from(card.older_claim.as_str())?;
            let newer = ClaimId::try_from(card.newer_claim.as_str())?;
            for later in &card.later_judgments {
                if later.relation != authoritative.relation {
                    warnings.push(crate::relate::settlement::AuthorityWarning {
                        old_claim: older.clone(),
                        new_claim: newer.clone(),
                        prior_verdict: authoritative.relation,
                        prior_fingerprint: authoritative.judge_fingerprint.clone(),
                        new_verdict: later.relation,
                        new_fingerprint: later.judge_fingerprint.clone(),
                        message: "authoritative verdict unchanged".to_string(),
                    });
                }
            }
            settled.insert(
                (older.clone(), newer.clone()),
                crate::semantics::RelationVerdict {
                    relation: authoritative.relation.into(),
                    score: ppm_to_unit_interval(authoritative.score_ppm),
                },
            );
            cache_keys.insert(
                (older.to_string(), newer.to_string()),
                authoritative.cache_key_hex.clone(),
            );
        }
        Ok::<_, TexoError>(SettlementAuthority {
            verdicts: settled,
            cache_keys,
            warnings,
            unresolved_pairs,
        })
    })?
}

fn settlement_entities(
    store: &crate::journal_store::JournalStore,
    workspace_id: &str,
    frontier: Option<u64>,
) -> BTreeSet<String> {
    let region = Region::scope(scope_for_workspace(workspace_id));
    let mut after = None;
    let mut entities = BTreeSet::new();
    loop {
        let page = store.query_entries_after(&region, after, 256);
        if page.is_empty() {
            break;
        }
        for entry in &page {
            if frontier.is_some_and(|limit| entry.global_sequence() > limit) {
                break;
            }
            if entry.coord().entity().starts_with("relation:") {
                entities.insert(entry.coord().entity().to_string());
            }
        }
        if page
            .last()
            .is_some_and(|entry| frontier.is_some_and(|limit| entry.global_sequence() > limit))
        {
            break;
        }
        after = page.last().map(batpak::store::IndexEntry::global_sequence);
    }
    entities
}

#[derive(Clone, Copy)]
pub(super) struct RejudgePairContext<'a> {
    pub(super) op: &'static str,
    pub(super) root: &'a Path,
    pub(super) gateway: Option<&'a crate::gateway::GatewayConfig>,
    pub(super) workspace_id: &'a WorkspaceId,
    pub(super) claims: &'a [(ClaimId, SemanticClaimView)],
    pub(super) authority: &'a SettlementAuthority,
    pub(super) requested: &'a (ClaimId, ClaimId),
    pub(super) observed_at_ms: u64,
}

pub(super) fn rejudge_authoritative_pair(
    cx: &mut syncbat::Ctx<'_>,
    context: RejudgePairContext<'_>,
) -> Result<RejudgedPairRow, TexoError> {
    let RejudgePairContext {
        op,
        root,
        gateway,
        workspace_id,
        claims,
        authority,
        requested,
        observed_at_ms,
    } = context;
    let (older, newer, prior) = authority
        .verdicts
        .get(requested)
        .map(|verdict| (requested.0.clone(), requested.1.clone(), *verdict))
        .or_else(|| {
            let reversed = (requested.1.clone(), requested.0.clone());
            authority
                .verdicts
                .get(&reversed)
                .map(|verdict| (reversed.0, reversed.1, *verdict))
        })
        .ok_or_else(|| TexoError::OpInput {
            op: op.to_string(),
            detail: format!(
                "rejudge pair {} {} has no journal-authoritative judgment",
                requested.0, requested.1
            ),
        })?;
    let by_id = claims
        .iter()
        .map(|(id, view)| (id.as_str(), view))
        .collect::<BTreeMap<_, _>>();
    let old_view = by_id
        .get(older.as_str())
        .ok_or_else(|| TexoError::OpInput {
            op: op.to_string(),
            detail: format!("rejudge older claim {older} is not current semantic input"),
        })?;
    let new_view = by_id
        .get(newer.as_str())
        .ok_or_else(|| TexoError::OpInput {
            op: op.to_string(),
            detail: format!("rejudge newer claim {newer} is not current semantic input"),
        })?;
    let fresh = fresh_pair_judgment(root, gateway, &old_view.text, &new_view.text)?;
    append_json(
        op,
        cx,
        <RelationJudgedV1 as EventPayload>::KIND,
        &RelationJudgedV1 {
            workspace_id: workspace_id.clone(),
            older_claim: older.clone(),
            newer_claim: newer.clone(),
            relation: fresh.verdict.relation.into(),
            score_ppm: unit_interval_to_ppm(fresh.verdict.score),
            judge_fingerprint: fresh.judge_fingerprint.clone(),
            cache_key_hex: fresh.cache_key.clone(),
            observed_at_ms,
        },
    )?;
    Ok(RejudgedPairRow {
        older_claim: older.to_string(),
        newer_claim: newer.to_string(),
        prior_relation: prior.relation,
        fresh_relation: fresh.verdict.relation,
        score_ppm: unit_interval_to_ppm(fresh.verdict.score),
        judge_fingerprint: fresh.judge_fingerprint,
        cache_key: fresh.cache_key,
    })
}

struct FreshPairJudgment {
    verdict: crate::semantics::RelationVerdict,
    judge_fingerprint: String,
    cache_key: String,
}

#[cfg(feature = "openrouter")]
fn fresh_pair_judgment(
    root: &Path,
    gateway: Option<&crate::gateway::GatewayConfig>,
    older: &str,
    newer: &str,
) -> Result<FreshPairJudgment, TexoError> {
    use crate::extract::cache::CachingRelater;
    use crate::semantics::openrouter::OpenRouterRelater;
    use crate::semantics::ClaimRelater as _;

    let cache_dir = std::env::var_os(ENV_RELATE_CACHE)
        .map_or_else(|| root.join(DEFAULT_RELATE_CACHE), PathBuf::from);
    let relater = CachingRelater::new(
        OpenRouterRelater::new(None, gateway).map_err(semantic_error)?,
        cache_dir,
    );
    let cache_key = relater.cache_key(older, newer);
    relater
        .evict(older, newer)
        .map_err(|error| TexoError::Semantics {
            backend: "relate-cache".to_string(),
            detail: format!("cannot evict {cache_key}: {error}"),
        })?;
    let verdict = relater.relate(older, newer).map_err(semantic_error)?;
    Ok(FreshPairJudgment {
        verdict,
        judge_fingerprint: relater.fingerprint(),
        cache_key,
    })
}

#[cfg(not(feature = "openrouter"))]
fn fresh_pair_judgment(
    _root: &Path,
    _gateway: Option<&crate::gateway::GatewayConfig>,
    _older: &str,
    _newer: &str,
) -> Result<FreshPairJudgment, TexoError> {
    Err(TexoError::Semantics {
        backend: "openrouter".to_string(),
        detail: "openrouter feature is disabled".to_string(),
    })
}
