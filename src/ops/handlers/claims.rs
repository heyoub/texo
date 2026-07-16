use super::common::{
    append_json, assemble_snapshot_view, claim_receipt, claim_record_receipts,
    claim_timeline_through, coverage_for_view, evidence_projection_through, op_runtime,
    parse_input, run_op, take_one_receipt, WORKSPACE_VIEW_PROJECTION,
};
use super::knowledge_read::{answer_state_for_claim, load_code_artifact_at, merge_coverage};
use super::model::{AgentClaimRow, AgentSourceRow};
use super::stats::claim_phase_name;
use crate::claims::card::ClaimCard;
use crate::claims::timeline::TimelineEntry;
use crate::claims::workspace::WorkspaceView;
use crate::error::TexoError;
use crate::events::coordinate::entity_for_claim;
use crate::events::machines::{transition_record, TransitionCauseV1, CLAIM_MACHINE};
use crate::events::payloads::ClaimSupersededV2;
use crate::knowledge::{
    AnswerState, ClaimEvidence, CodeOccurrence, CoverageGap, CoverageGapKind, KnowledgeCoverage,
    SnapshotRead,
};
use crate::ops::env;
use crate::ops::env::ReceiptNote;
use batpak::event::EventPayload;
use batpak::store::Freshness;
use serde::{Deserialize, Serialize};
use syncbat::HandlerResult;

const CLAIM_EXPLAIN_PROJECTION: &str = "texo.claim.explain.v2";

#[syncbat::operation(
    descriptor = CLAIMS_LIST,
    register = register_claims_list,
    register_item = claims_list_item,
    name = "texo.claims.list",
    effect = Inspect,
    input_schema = "texo.claims.list.input.v3",
    output_schema = "texo.claims.list.output.v3",
    receipt_kind = "receipt.texo.claims.list.v3",
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn claims_list(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.claims.list", || {
        let input: ClaimsListInput = parse_input("texo.claims.list", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.claims.list", error))?;
        let (view, snapshot) = assemble_snapshot_view(input.snapshot.as_deref())?;
        let claims = claim_list_rows(&view, input.subject.as_deref())?;
        Ok(ClaimsListOutput {
            workspace_id: view.workspace_id.clone(),
            frontier: view.frontier,
            claims,
            snapshot,
        })
    })
}
#[syncbat::operation(
    descriptor = CLAIMS_SEARCH,
    register = register_claims_search,
    register_item = claims_search_item,
    name = "texo.claims.search",
    effect = Inspect,
    input_schema = "texo.claims.search.input.v2",
    output_schema = "texo.claims.search.output.v2",
    receipt_kind = "receipt.texo.claims.search.v2",
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn claims_search(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.claims.search", || {
        let input: ClaimsSearchInput = parse_input("texo.claims.search", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.claims.search", error))?;
        let (view, snapshot) = assemble_snapshot_view(input.snapshot.as_deref())?;
        let query = input.query.unwrap_or_default();
        if query.len() > 256 {
            return Err(TexoError::OpInput {
                op: "texo.claims.search".to_string(),
                detail: "query exceeds 256 bytes".to_string(),
            });
        }
        let limit = input.limit.unwrap_or(25);
        if !(1..=100).contains(&limit) {
            return Err(TexoError::OpInput {
                op: "texo.claims.search".to_string(),
                detail: "limit must be between 1 and 100".to_string(),
            });
        }
        let offset = parse_claim_search_cursor(input.cursor.as_deref())?;
        let query_terms = query
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        let rows = claim_list_rows(&view, input.subject.as_deref())?
            .into_iter()
            .filter(|row| input.status.is_none_or(|status| row.status == status))
            .filter(|row| claim_matches_query(row, &query_terms))
            .collect::<Vec<_>>();
        let total = rows.len();
        let page = rows
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        let returned = page.len();
        let next_offset = offset.saturating_add(returned);
        let has_more = next_offset < total;
        Ok(ClaimsSearchOutput {
            workspace_id: view.workspace_id.clone(),
            frontier: view.frontier,
            freshness: view.freshness,
            total,
            returned,
            has_more,
            next_cursor: has_more.then(|| format!("texo-claims-v1:{next_offset}")),
            claims: page,
            snapshot,
        })
    })
}
#[syncbat::operation(
    descriptor = KNOWLEDGE_SEARCH,
    register = register_knowledge_search,
    register_item = knowledge_search_item,
    name = "texo.knowledge.search",
    effect = Inspect,
    input_schema = "texo.knowledge.search.input.v1",
    output_schema = "texo.knowledge.search.output.v1",
    receipt_kind = "receipt.texo.knowledge.search.v1",
    reads_events = ["evt.e00e"],
    queries_projections = ["texo.workspace.view.v2", "texo.code.index.v1"]
)]
#[tracing::instrument(skip_all)]
fn knowledge_search(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.knowledge.search", || {
        let input: KnowledgeSearchInput = parse_input("texo.knowledge.search", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.knowledge.search", error))?;
        let (view, snapshot) = assemble_snapshot_view(input.snapshot.as_deref())?;
        search_knowledge_from_view(&view, snapshot, &input)
    })
}
#[syncbat::operation(
    descriptor = CLAIM_EXPLAIN,
    register = register_claim_explain,
    register_item = claim_explain_item,
    name = "texo.claim.explain",
    effect = Inspect,
    input_schema = "texo.claim.explain.input.v3",
    output_schema = "texo.claim.explain.output.v4",
    receipt_kind = "receipt.texo.claim.explain.v4",
    queries_projections = ["texo.claim.explain.v2"]
)]
#[tracing::instrument(skip_all)]
fn claim_explain(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.claim.explain", || {
        let input: ClaimExplainInput = parse_input("texo.claim.explain", input)?;
        cx.projection_read_handle()
            .query_projection(CLAIM_EXPLAIN_PROJECTION)
            .map_err(|error| op_runtime("texo.claim.explain", error))?;
        let entity = entity_for_claim(&input.claim_id);
        let (view, snapshot) = assemble_snapshot_view(input.snapshot.as_deref())?;
        let card = view
            .claims
            .iter()
            .find(|claim| claim.card.claim_id == input.claim_id)
            .map(|claim| claim.card.as_ref().clone())
            .ok_or_else(|| TexoError::MissingEntity {
                entity: entity.clone(),
            })?;
        let timeline = claim_timeline_through(&entity, view.frontier)?;
        let evidence = evidence_projection_through(view.frontier)?.take_claim(&input.claim_id);
        let coverage = coverage_for_view(&view, &snapshot)?;
        let answer_state = answer_state_for_claim(
            view.claims
                .iter()
                .find(|claim| claim.card.claim_id == input.claim_id)
                .map(|claim| claim.status),
            &evidence,
        );
        Ok(ClaimExplainOutput {
            card,
            timeline: timeline.entries,
            answer_state,
            evidence,
            coverage,
            snapshot,
        })
    })
}
#[syncbat::operation(
    descriptor = CLAIM_SUPERSEDE,
    register = register_claim_supersede,
    register_item = claim_supersede_item,
    name = "texo.claim.supersede",
    effect = Persist,
    input_schema = "texo.claim.supersede.input.v2",
    output_schema = "texo.claim.supersede.output.v2",
    receipt_kind = "receipt.texo.claim.supersede.v2",
    appends_events = ["evt.e003"],
    queries_projections = ["texo.workspace.view.v2"]
)]
#[tracing::instrument(skip_all)]
fn claim_supersede(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.claim.supersede", || {
        let input: ClaimSupersedeInput = parse_input("texo.claim.supersede", input)?;
        if input.old == input.new {
            return Err(TexoError::OpInput {
                op: "texo.claim.supersede".to_string(),
                detail: "old and new claims must differ".to_string(),
            });
        }
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.claim.supersede", error))?;

        let old_entity = entity_for_claim(&input.old);
        let new_entity = entity_for_claim(&input.new);
        let (old_card, new_card, workspace_id) = env::with(|op_env| {
            let old_card = env::deterministic_projection(|| {
                op_env
                    .store
                    .project::<ClaimCard>(&old_entity, &Freshness::Consistent)
            })?;
            let new_card = env::deterministic_projection(|| {
                op_env
                    .store
                    .project::<ClaimCard>(&new_entity, &Freshness::Consistent)
            })?;
            Ok::<_, TexoError>((old_card, new_card, op_env.workspace_id.clone()))
        })??;
        let old_card = old_card.ok_or_else(|| TexoError::MissingEntity {
            entity: old_entity.clone(),
        })?;
        let _new_card = new_card.ok_or_else(|| TexoError::MissingEntity {
            entity: new_entity.clone(),
        })?;
        if old_card.phase == 2 && old_card.superseded_by.as_deref() == Some(input.new.as_str()) {
            return Ok(ClaimSupersedeOutput {
                old: input.old,
                new: input.new,
                already_applied: true,
                receipt: None,
            });
        }
        if old_card.phase != 1 {
            return Err(TexoError::Transition {
                machine: CLAIM_MACHINE.to_string(),
                from: old_card.phase,
                to: 2,
                context: Some(format!(
                    "claim {} is already {}{}",
                    input.old,
                    claim_phase_name(old_card.phase),
                    old_card
                        .superseded_by
                        .as_deref()
                        .map_or_else(String::new, |successor| format!(" by {successor}"))
                )),
            });
        }

        let payload = ClaimSupersededV2 {
            old_claim_id: input.old.clone(),
            new_claim_id: input.new.clone(),
            workspace_id,
            reason: input.reason,
            decided_by: input.decided_by,
            observed_at_ms: input.observed_at_ms,
            transition: transition_record(
                CLAIM_MACHINE,
                &old_entity,
                1,
                2,
                vec![TransitionCauseV1 {
                    lane: 0,
                    key: format!("claim:{}", input.new),
                }],
                input.observed_at_ms,
            ),
        };
        append_json(
            "texo.claim.supersede",
            cx,
            <ClaimSupersededV2 as EventPayload>::KIND,
            &payload,
        )?;
        Ok(ClaimSupersedeOutput {
            old: input.old,
            new: input.new,
            already_applied: false,
            receipt: Some(take_one_receipt("texo.claim.supersede")?),
        })
    })
}
#[derive(Debug, Deserialize)]
struct ClaimsListInput {
    subject: Option<String>,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaimsSearchInput {
    query: Option<String>,
    subject: Option<String>,
    status: Option<crate::claims::status::ClaimStatus>,
    limit: Option<usize>,
    cursor: Option<String>,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
struct ClaimsListOutput {
    workspace_id: String,
    frontier: u64,
    claims: Vec<AgentClaimRow>,
    snapshot: SnapshotRead,
}

#[derive(Debug, Serialize)]
struct ClaimsSearchOutput {
    workspace_id: String,
    frontier: u64,
    freshness: crate::claims::workspace::ProjectionFreshness,
    total: usize,
    returned: usize,
    has_more: bool,
    next_cursor: Option<String>,
    claims: Vec<AgentClaimRow>,
    snapshot: SnapshotRead,
}

#[derive(Debug, Deserialize)]
struct KnowledgeSearchInput {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    status: Option<crate::claims::status::ClaimStatus>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
struct KnowledgeSearchOutput {
    workspace_id: String,
    frontier: u64,
    total: usize,
    returned: usize,
    has_more: bool,
    next_cursor: Option<String>,
    results: Vec<KnowledgeSearchResult>,
    code_index_available: bool,
    coverage: KnowledgeCoverage,
    snapshot: SnapshotRead,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum KnowledgeSearchResult {
    Claim { claim: AgentClaimRow },
    Code { occurrence: CodeOccurrence },
}

struct RankedKnowledgeResult {
    rank: u8,
    key: String,
    result: KnowledgeSearchResult,
}

#[derive(Debug, Deserialize)]
struct ClaimExplainInput {
    claim_id: String,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Debug, Serialize)]
struct ClaimExplainOutput {
    card: ClaimCard,
    timeline: Vec<TimelineEntry>,
    answer_state: AnswerState,
    evidence: Vec<ClaimEvidence>,
    coverage: KnowledgeCoverage,
    snapshot: SnapshotRead,
}
#[derive(Debug, Deserialize)]
struct ClaimSupersedeInput {
    old: String,
    new: String,
    reason: String,
    decided_by: String,
    observed_at_ms: u64,
}

#[derive(Debug, Serialize)]
struct ClaimSupersedeOutput {
    old: String,
    new: String,
    already_applied: bool,
    receipt: Option<ReceiptNote>,
}

pub(super) fn claim_list_rows(
    view: &WorkspaceView,
    subject: Option<&str>,
) -> Result<Vec<AgentClaimRow>, TexoError> {
    let mut rows = Vec::new();
    // One index scan for all receipts instead of one by_entity per claim.
    let receipts = claim_record_receipts()?;
    for claim in &view.claims {
        if subject.is_some_and(|wanted| claim.card.subject_hint.as_deref() != Some(wanted)) {
            continue;
        }
        let receipt = match receipts.get(&claim.card.claim_id) {
            Some(row) => row.clone(),
            None => claim_receipt(&claim.card.claim_id)?,
        };
        rows.push(AgentClaimRow {
            claim_id: claim.card.claim_id.clone(),
            status: claim.status,
            subject_hint: claim.card.subject_hint.clone(),
            text: claim.card.text.clone(),
            source: AgentSourceRow {
                source_id: claim.card.source_id.clone(),
                path: claim.card.source_path.clone(),
                line_start: claim.card.line_start,
            },
            receipt,
            supersedes: claim.supersedes.clone(),
            superseded_by: claim.card.superseded_by.clone(),
        });
    }
    Ok(rows)
}

fn search_knowledge_from_view(
    view: &WorkspaceView,
    snapshot: SnapshotRead,
    input: &KnowledgeSearchInput,
) -> Result<KnowledgeSearchOutput, TexoError> {
    let (query, limit) = validated_knowledge_search(input)?;
    let cursor_identity = knowledge_search_identity(&snapshot, query, input);
    let offset = parse_knowledge_search_cursor(input.cursor.as_deref(), &cursor_identity)?;
    let query_lower = query.to_ascii_lowercase();
    let mut ranked = claim_list_rows(view, input.subject.as_deref())?
        .into_iter()
        .filter(|claim| input.status.is_none_or(|status| claim.status == status))
        .filter_map(|claim| {
            claim_search_rank(&claim, &query_lower).map(|rank| RankedKnowledgeResult {
                rank,
                key: format!("claim:{}", claim.claim_id),
                result: KnowledgeSearchResult::Claim { claim },
            })
        })
        .collect::<Vec<_>>();
    let mut coverage = coverage_for_view(view, &snapshot)?;
    // A subject/status filter searches only the claim plane; the code plane is
    // deliberately not consulted, which is "not searched", not "unavailable".
    let code_searched = input.subject.is_none() && input.status.is_none();
    let mut code_index_available = false;
    if code_searched {
        if let Some(source_snapshot_id) = snapshot.descriptor.source_snapshot_id.as_ref() {
            let loaded = load_code_artifact_at(view.frontier, source_snapshot_id)?;
            if let Some(code_coverage) = &loaded.coverage {
                merge_coverage(&mut coverage, code_coverage);
            }
            if let Some(artifact) = loaded.artifact {
                code_index_available = true;
                ranked.extend(artifact.occurrences.into_iter().filter_map(|occurrence| {
                    code_search_rank(&occurrence, &query_lower).map(|rank| RankedKnowledgeResult {
                        rank,
                        key: format!(
                            "code:{}:{}:{}",
                            occurrence.symbol, occurrence.path, occurrence.byte_range.start
                        ),
                        result: KnowledgeSearchResult::Code { occurrence },
                    })
                }));
            }
        }
    }
    // Only assert the code index is unavailable when it was actually searched and
    // found absent — never for a filtered search that skipped it by design.
    if code_searched
        && !code_index_available
        && !coverage
            .gaps
            .iter()
            .any(|gap| gap.kind == CoverageGapKind::CodeIndexUnavailable)
    {
        coverage.gaps.push(CoverageGap {
            path: None,
            kind: CoverageGapKind::CodeIndexUnavailable,
        });
    }
    ranked.sort_by(|left, right| {
        left.rank
            .cmp(&right.rank)
            .then_with(|| left.key.cmp(&right.key))
    });
    let total = ranked.len();
    let results = ranked
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|ranked| ranked.result)
        .collect::<Vec<_>>();
    let returned = results.len();
    let next_offset = offset.saturating_add(returned);
    let has_more = next_offset < total;
    Ok(KnowledgeSearchOutput {
        workspace_id: view.workspace_id.clone(),
        frontier: view.frontier,
        total,
        returned,
        has_more,
        next_cursor: has_more.then(|| format!("texo-knowledge-v1:{cursor_identity}:{next_offset}")),
        results,
        code_index_available,
        coverage,
        snapshot,
    })
}

fn validated_knowledge_search(input: &KnowledgeSearchInput) -> Result<(&str, usize), TexoError> {
    let query = input.query.as_deref().unwrap_or("");
    if query.len() > 256 {
        return Err(TexoError::OpInput {
            op: "texo.knowledge.search".to_string(),
            detail: "query exceeds 256 bytes".to_string(),
        });
    }
    let limit = input.limit.unwrap_or(25);
    if !(1..=100).contains(&limit) {
        return Err(TexoError::OpInput {
            op: "texo.knowledge.search".to_string(),
            detail: "limit must be between 1 and 100".to_string(),
        });
    }
    Ok((query, limit))
}

fn knowledge_search_identity(
    snapshot: &SnapshotRead,
    query: &str,
    input: &KnowledgeSearchInput,
) -> String {
    let material = format!(
        "texo.knowledge.search.v1\u{1f}{}\u{1f}{query}\u{1f}{}\u{1f}{}",
        snapshot.token.as_str(),
        input.subject.as_deref().unwrap_or(""),
        input
            .status
            .map_or("", crate::claims::status::ClaimStatus::as_str)
    );
    blake3::hash(material.as_bytes()).to_hex()[..16].to_string()
}

fn parse_knowledge_search_cursor(
    cursor: Option<&str>,
    expected_identity: &str,
) -> Result<usize, TexoError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let Some(rest) = cursor.strip_prefix("texo-knowledge-v1:") else {
        return Err(invalid_knowledge_cursor());
    };
    let Some((identity, offset)) = rest.split_once(':') else {
        return Err(invalid_knowledge_cursor());
    };
    if identity != expected_identity {
        return Err(invalid_knowledge_cursor());
    }
    offset.parse().map_err(|_| invalid_knowledge_cursor())
}

fn invalid_knowledge_cursor() -> TexoError {
    TexoError::OpInput {
        op: "texo.knowledge.search".to_string(),
        detail: "cursor is invalid for this query and snapshot".to_string(),
    }
}

fn claim_search_rank(claim: &AgentClaimRow, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    let text = claim.text.to_ascii_lowercase();
    let subject = claim
        .subject_hint
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let path = claim.source.path.to_ascii_lowercase();
    if text == query || subject == query {
        Some(0)
    } else if text.starts_with(query) || subject.starts_with(query) {
        Some(1)
    } else if text.contains(query) || subject.contains(query) {
        Some(2)
    } else if path.contains(query) {
        Some(3)
    } else {
        None
    }
}

fn code_search_rank(occurrence: &CodeOccurrence, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(1);
    }
    let name = occurrence.display_name.to_ascii_lowercase();
    let symbol = occurrence.symbol.to_ascii_lowercase();
    let path = occurrence.path.to_ascii_lowercase();
    if name == query || symbol == query {
        Some(0)
    } else if name.starts_with(query) {
        Some(1)
    } else if name.contains(query) || symbol.contains(query) {
        Some(2)
    } else if path.contains(query) {
        Some(3)
    } else {
        None
    }
}

pub(super) fn parse_claim_search_cursor(cursor: Option<&str>) -> Result<usize, TexoError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let Some(offset) = cursor.strip_prefix("texo-claims-v1:") else {
        return Err(TexoError::OpInput {
            op: "texo.claims.search".to_string(),
            detail: "cursor has an unsupported schema".to_string(),
        });
    };
    offset.parse::<usize>().map_err(|_| TexoError::OpInput {
        op: "texo.claims.search".to_string(),
        detail: "cursor offset is invalid".to_string(),
    })
}

pub(super) fn claim_matches_query(row: &AgentClaimRow, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let mut searchable = row.text.to_ascii_lowercase();
    searchable.push(' ');
    searchable.push_str(&row.source.path.to_ascii_lowercase());
    if let Some(subject) = &row.subject_hint {
        searchable.push(' ');
        searchable.push_str(&subject.to_ascii_lowercase());
    }
    terms.iter().all(|term| searchable.contains(term))
}
