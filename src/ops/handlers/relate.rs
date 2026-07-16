pub(super) use self::authority::authoritative_settlements;
use self::authority::{rejudge_authoritative_pair, RejudgePairContext};
use self::backend::{relate_with_backends, semantic_claims_from_view, RelateBackendContext};
use self::campaign::{
    append_checkpoint, candidate_policy_digest, latest_checkpoint, resolve_start, CampaignBasis,
    CampaignStart, CheckpointDraft,
};
pub(super) use self::campaign::{require_complete_settlement, settlement_is_complete};
use self::publication::{
    append_relate_conflicts, append_relate_supersessions, append_relation_deferrals,
    append_relation_judgments, relate_publication,
};
use super::common::{
    assemble_current_view, op_runtime, parse_input, run_op, semantic_temporal_policy,
    take_receipts, WORKSPACE_VIEW_PROJECTION,
};
use crate::claims::workspace::WorkspaceView;
use crate::error::TexoError;
use crate::events::ids::{ClaimId, WorkspaceId};
use crate::ops::env;
use crate::ops::env::ReceiptNote;
use crate::relate::settlement::CampaignPhase;
use crate::semantics::pipeline::{
    CandidateCursor, ClaimView as SemanticClaimView, RelateOutcome, RelateTemporalPolicy,
    RelateThresholds, DEFAULT_CANDIDATE_PAIR_BUDGET,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use syncbat::HandlerResult;

const RELATE_PREFILTER: f32 = 0.60;
#[cfg(feature = "openrouter")]
pub(super) const ENV_RELATE_CACHE: &str = "TEXO_RELATE_CACHE";
#[cfg(feature = "openrouter")]
pub(super) const DEFAULT_RELATE_CACHE: &str = ".texo/relate-cache";

mod authority;
mod backend;
mod campaign;
mod publication;
#[syncbat::operation(
    descriptor = RELATE_RUN,
    register = register_relate_run,
    register_item = relate_run_item,
    name = "texo.relate.run",
    effect = Persist,
    input_schema = "texo.relate.run.input.v2",
    output_schema = "texo.relate.run.output.v2",
    receipt_kind = "receipt.texo.relate.run.v2",
    appends_events = ["evt.e003", "evt.e004", "evt.e009", "evt.e00a", "evt.e012"],
    queries_projections = ["texo.workspace.view.v2"],
    requires_capabilities = ["texo.cap.model"]
)]
#[tracing::instrument(skip_all)]
fn relate_run(input: &[u8], cx: &mut syncbat::Ctx<'_>) -> HandlerResult {
    run_op("texo.relate.run", || {
        let input: RelateRunInput = parse_input("texo.relate.run", input)?;
        cx.projection_read_handle()
            .query_projection(WORKSPACE_VIEW_PROJECTION)
            .map_err(|error| op_runtime("texo.relate.run", error))?;
        run_relate_pass(
            "texo.relate.run",
            cx,
            input.observed_at_ms,
            input.validated_options()?,
        )
    })
}

#[derive(Debug, Deserialize)]
struct RelateRunInput {
    observed_at_ms: u64,
    #[serde(default)]
    strict: bool,
    #[serde(default)]
    max_candidate_pairs: Option<usize>,
    #[serde(default)]
    candidate_cursor: Option<u64>,
    #[serde(default)]
    rejudge_pair: Option<Vec<String>>,
}

impl RelateRunInput {
    fn validated_options(&self) -> Result<RelatePassOptions, TexoError> {
        let candidate_pair_budget = self
            .max_candidate_pairs
            .unwrap_or(DEFAULT_CANDIDATE_PAIR_BUDGET);
        if candidate_pair_budget == 0 {
            return Err(TexoError::OpInput {
                op: "texo.relate.run".to_string(),
                detail: "max_candidate_pairs must be greater than zero".to_string(),
            });
        }
        let rejudge_pair = match self.rejudge_pair.as_deref() {
            None => None,
            Some([older, newer]) => Some((
                ClaimId::try_from(older.as_str())?,
                ClaimId::try_from(newer.as_str())?,
            )),
            Some(_) => {
                return Err(TexoError::OpInput {
                    op: "texo.relate.run".to_string(),
                    detail: "rejudge_pair must contain exactly two claim ids".to_string(),
                });
            }
        };
        Ok(RelatePassOptions {
            strict: self.strict,
            candidate_pair_budget,
            candidate_cursor: self.candidate_cursor.map(CandidateCursor::from_offset),
            rejudge_pair,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RelatePassOptions {
    strict: bool,
    candidate_pair_budget: usize,
    candidate_cursor: Option<CandidateCursor>,
    rejudge_pair: Option<(ClaimId, ClaimId)>,
}

impl RelatePassOptions {
    pub(crate) fn best_effort() -> Self {
        Self {
            strict: false,
            candidate_pair_budget: DEFAULT_CANDIDATE_PAIR_BUDGET,
            candidate_cursor: None,
            rejudge_pair: None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum RelateCompletion {
    Complete,
    Partial,
}

#[derive(Debug, Serialize)]
pub(crate) struct RelateRunOutput {
    outcome: RelateCompletion,
    pub(crate) claims_related: usize,
    pub(crate) supersessions: Vec<RelateSupersessionRow>,
    pub(crate) conflicts: Vec<RelateConflictRow>,
    unresolved: Vec<crate::relate::settlement::UnresolvedPair>,
    held: Vec<crate::relate::settlement::HeldDecision>,
    warnings: Vec<String>,
    authority_warnings: Vec<crate::relate::settlement::AuthorityWarning>,
    candidate_pairs: usize,
    candidate_pair_budget: usize,
    next_candidate_cursor: Option<u64>,
    rejudged_pair: Option<RejudgedPairRow>,
    pub(crate) receipts: Vec<ReceiptNote>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RelateSupersessionRow {
    old_claim_id: String,
    new_claim_id: String,
    reason: String,
    cache_key: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct RelateConflictRow {
    conflict_id: String,
    claim_a: String,
    claim_b: String,
    reason: String,
    cache_key: String,
}

#[derive(Debug, Serialize)]
struct RejudgedPairRow {
    older_claim: String,
    newer_claim: String,
    prior_relation: crate::semantics::ClaimRelation,
    fresh_relation: crate::semantics::ClaimRelation,
    score_ppm: u32,
    judge_fingerprint: String,
    cache_key: String,
}

pub(crate) fn run_relate_pass(
    op: &'static str,
    cx: &mut syncbat::Ctx<'_>,
    observed_at_ms: u64,
    options: RelatePassOptions,
) -> Result<RelateRunOutput, TexoError> {
    let prepared = prepare_relate_pass(options)?;
    match prepared.execution {
        CampaignExecution::NoopComplete => {
            let mut output =
                empty_relate_output(prepared.claims.len(), prepared.candidate_pair_budget);
            output.receipts = take_receipts()?;
            return Ok(output);
        }
        CampaignExecution::RejudgeComplete => {
            return rejudge_completed_campaign(op, cx, observed_at_ms, prepared);
        }
        CampaignExecution::Run(_) => {}
    }
    if prepared.rejudge_pair.is_some() && prepared.claims.len() < 2 {
        return Err(TexoError::OpInput {
            op: op.to_string(),
            detail: "rejudge pair is no longer present in the current semantic claim set"
                .to_string(),
        });
    }
    if prepared.claims.len() < 2 {
        return complete_trivial_campaign(op, cx, observed_at_ms, prepared);
    }
    let evaluated = evaluate_relate_pass(op, cx, observed_at_ms, &prepared)?;
    publish_relate_pass(op, cx, observed_at_ms, prepared, evaluated)
}

struct PreparedRelatePass {
    strict: bool,
    candidate_pair_budget: usize,
    candidate_cursor: CandidateCursor,
    rejudge_pair: Option<(ClaimId, ClaimId)>,
    view: Arc<WorkspaceView>,
    claims: Vec<(ClaimId, SemanticClaimView)>,
    settings: RelateSettings,
    evaluated_basis: CampaignBasis,
    evaluated_basis_digest: String,
    policy_digest: String,
    execution: CampaignExecution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CampaignExecution {
    NoopComplete,
    RejudgeComplete,
    Run(CandidateCursor),
}

fn campaign_execution(start: CampaignStart, rejudge_requested: bool) -> CampaignExecution {
    match (start, rejudge_requested) {
        (CampaignStart::AlreadyComplete, false) => CampaignExecution::NoopComplete,
        (CampaignStart::AlreadyComplete, true) => CampaignExecution::RejudgeComplete,
        (CampaignStart::Run(cursor), _) => CampaignExecution::Run(cursor),
    }
}

fn prepare_relate_pass(options: RelatePassOptions) -> Result<PreparedRelatePass, TexoError> {
    let RelatePassOptions {
        strict,
        candidate_pair_budget,
        candidate_cursor,
        rejudge_pair,
    } = options;
    let view = assemble_current_view()?;
    let claims = semantic_claims_from_view(&view)?;
    let settings = relate_settings(&view)?;
    let evaluated_basis = CampaignBasis::from_view(&view, &settings.temporal);
    let evaluated_basis_digest = evaluated_basis.digest();
    let policy_digest = candidate_policy_digest(
        settings.cluster,
        settings.prefilter,
        settings.gateway.as_ref(),
    );
    let campaign_start = resolve_start(
        latest_checkpoint(&view)?.as_ref(),
        &evaluated_basis_digest,
        &policy_digest,
        candidate_cursor,
    )?;
    let execution = campaign_execution(campaign_start, rejudge_pair.is_some());
    let candidate_cursor = match execution {
        CampaignExecution::Run(cursor) => cursor,
        CampaignExecution::NoopComplete | CampaignExecution::RejudgeComplete => {
            CandidateCursor::start()
        }
    };
    Ok(PreparedRelatePass {
        strict,
        candidate_pair_budget,
        candidate_cursor,
        rejudge_pair,
        view,
        claims,
        settings,
        evaluated_basis,
        evaluated_basis_digest,
        policy_digest,
        execution,
    })
}

fn rejudge_completed_campaign(
    op: &'static str,
    cx: &mut syncbat::Ctx<'_>,
    observed_at_ms: u64,
    prepared: PreparedRelatePass,
) -> Result<RelateRunOutput, TexoError> {
    let requested = prepared
        .rejudge_pair
        .as_ref()
        .ok_or_else(|| TexoError::OpInput {
            op: op.to_string(),
            detail: "completed-campaign rejudge requires a claim pair".to_string(),
        })?;
    let authority = authoritative_settlements(None)?;
    let rejudged_pair = rejudge_authoritative_pair(
        cx,
        RejudgePairContext {
            op,
            root: &prepared.settings.root,
            gateway: prepared.settings.gateway.as_ref(),
            workspace_id: &prepared.settings.workspace_id,
            claims: &prepared.claims,
            authority: &authority,
            requested,
            observed_at_ms,
        },
    )?;
    let authority = authoritative_settlements(None)?;
    append_checkpoint(
        op,
        cx,
        CheckpointDraft {
            workspace_id: prepared.settings.workspace_id,
            evaluated_basis_digest_hex: prepared.evaluated_basis_digest.clone(),
            result_basis_digest_hex: prepared.evaluated_basis_digest,
            candidate_policy_digest_hex: prepared.policy_digest,
            phase: CampaignPhase::Complete,
            observed_at_ms,
        },
    )?;
    let mut output = empty_relate_output(prepared.claims.len(), prepared.candidate_pair_budget);
    output.authority_warnings = authority.warnings;
    output.rejudged_pair = Some(rejudged_pair);
    output.receipts = take_receipts()?;
    Ok(output)
}

fn complete_trivial_campaign(
    op: &str,
    cx: &mut syncbat::Ctx<'_>,
    observed_at_ms: u64,
    prepared: PreparedRelatePass,
) -> Result<RelateRunOutput, TexoError> {
    append_checkpoint(
        op,
        cx,
        CheckpointDraft {
            workspace_id: prepared.settings.workspace_id,
            evaluated_basis_digest_hex: prepared.evaluated_basis_digest.clone(),
            result_basis_digest_hex: prepared.evaluated_basis_digest,
            candidate_policy_digest_hex: prepared.policy_digest,
            phase: CampaignPhase::Complete,
            observed_at_ms,
        },
    )?;
    let mut output = empty_relate_output(prepared.claims.len(), prepared.candidate_pair_budget);
    output.receipts = take_receipts()?;
    Ok(output)
}

struct EvaluatedRelatePass {
    related: backend::SemanticRelateOutput,
    authority_warnings: Vec<crate::relate::settlement::AuthorityWarning>,
    rejudged_pair: Option<RejudgedPairRow>,
}

fn evaluate_relate_pass(
    op: &'static str,
    cx: &mut syncbat::Ctx<'_>,
    observed_at_ms: u64,
    prepared: &PreparedRelatePass,
) -> Result<EvaluatedRelatePass, TexoError> {
    let mut authority = authoritative_settlements(None)?;
    let rejudged_pair = prepared
        .rejudge_pair
        .as_ref()
        .map(|pair| {
            rejudge_authoritative_pair(
                cx,
                RejudgePairContext {
                    op,
                    root: &prepared.settings.root,
                    gateway: prepared.settings.gateway.as_ref(),
                    workspace_id: &prepared.settings.workspace_id,
                    claims: &prepared.claims,
                    authority: &authority,
                    requested: pair,
                    observed_at_ms,
                },
            )
        })
        .transpose()?;
    if rejudged_pair.is_some() {
        authority = authoritative_settlements(None)?;
    }
    let mut related = relate_with_backends(RelateBackendContext {
        root: &prepared.settings.root,
        gateway: prepared.settings.gateway.as_ref(),
        claims: &prepared.claims,
        thresholds: RelateThresholds {
            cluster: prepared.settings.cluster,
            prefilter: prepared.settings.prefilter,
        },
        settled: &authority.verdicts,
        temporal: &prepared.settings.temporal,
        budget: std::time::Duration::from_secs(prepared.settings.budget_secs),
        candidate_pair_budget: prepared.candidate_pair_budget,
        candidate_cursor: prepared.candidate_cursor,
    })?;
    for (pair, cache_key) in &authority.cache_keys {
        related.cache_keys.insert(pair.clone(), cache_key.clone());
    }
    Ok(EvaluatedRelatePass {
        related,
        authority_warnings: authority.warnings,
        rejudged_pair,
    })
}

fn publish_relate_pass(
    op: &'static str,
    cx: &mut syncbat::Ctx<'_>,
    observed_at_ms: u64,
    prepared: PreparedRelatePass,
    evaluated: EvaluatedRelatePass,
) -> Result<RelateRunOutput, TexoError> {
    let related = evaluated.related;
    append_relation_judgments(
        op,
        cx,
        &prepared.settings.workspace_id,
        &related,
        observed_at_ms,
    )?;
    append_relation_deferrals(
        op,
        cx,
        &prepared.settings.workspace_id,
        related.outcome.unresolved(),
        observed_at_ms,
    )?;
    let publication = relate_publication(&related.outcome);
    let existing_conflicts = prepared
        .view
        .conflicts
        .iter()
        .map(|conflict| conflict.conflict_id.clone())
        .collect::<BTreeSet<_>>();
    let supersessions = append_relate_supersessions(
        op,
        cx,
        &prepared.view.workspace_id,
        &publication.supersessions,
        &related.cache_keys,
        observed_at_ms,
    )?;
    let conflicts = append_relate_conflicts(
        op,
        cx,
        &prepared.view.workspace_id,
        &publication.conflicts,
        &existing_conflicts,
        &related.cache_keys,
        observed_at_ms,
    )?;
    let (phase, result_basis_digest) = match &related.outcome {
        RelateOutcome::Complete(_) => (
            CampaignPhase::Complete,
            prepared
                .evaluated_basis
                .after_publication(&supersessions, &conflicts)
                .digest(),
        ),
        RelateOutcome::Partial(partial) => (
            CampaignPhase::Partial {
                next_candidate_cursor: partial.next_candidate_cursor.offset(),
            },
            prepared.evaluated_basis_digest.clone(),
        ),
    };
    append_checkpoint(
        op,
        cx,
        CheckpointDraft {
            workspace_id: prepared.settings.workspace_id,
            evaluated_basis_digest_hex: prepared.evaluated_basis_digest,
            result_basis_digest_hex: result_basis_digest,
            candidate_policy_digest_hex: prepared.policy_digest,
            phase,
            observed_at_ms,
        },
    )?;

    finish_relate_output(RelatePublicationOutput {
        claims_related: prepared.claims.len(),
        outcome: related.outcome,
        authority_warnings: evaluated.authority_warnings,
        supersessions,
        conflicts,
        rejudged_pair: evaluated.rejudged_pair,
        strict: prepared.strict,
    })
}

struct RelateSettings {
    root: PathBuf,
    cluster: f32,
    prefilter: f32,
    gateway: Option<crate::gateway::GatewayConfig>,
    temporal: RelateTemporalPolicy,
    budget_secs: u64,
    workspace_id: WorkspaceId,
}

fn relate_settings(view: &WorkspaceView) -> Result<RelateSettings, TexoError> {
    let (root, cluster, prefilter, gateway) = env::with(|op_env| {
        let semantics = op_env.config.semantics.as_ref();
        let cluster = semantics.map_or_else(
            || crate::config::SemanticsConfig::default().cosine_threshold,
            |config| config.cosine_threshold,
        );
        let prefilter = semantics
            .and_then(|config| config.relate_prefilter)
            .unwrap_or(RELATE_PREFILTER);
        (
            op_env.root.clone(),
            cluster,
            prefilter,
            op_env.config.gateway.clone(),
        )
    })?;
    let budget_secs = std::env::var("TEXO_RELATE_BUDGET_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| gateway.as_ref().map(|config| config.relate_budget_secs))
        .unwrap_or(900);
    Ok(RelateSettings {
        root,
        cluster,
        prefilter,
        gateway,
        temporal: semantic_temporal_policy(view)?,
        budget_secs,
        workspace_id: WorkspaceId::try_from(view.workspace_id.as_str())?,
    })
}

fn empty_relate_output(claims_related: usize, candidate_pair_budget: usize) -> RelateRunOutput {
    RelateRunOutput {
        outcome: RelateCompletion::Complete,
        claims_related,
        supersessions: Vec::new(),
        conflicts: Vec::new(),
        unresolved: Vec::new(),
        held: Vec::new(),
        warnings: Vec::new(),
        authority_warnings: Vec::new(),
        candidate_pairs: 0,
        candidate_pair_budget,
        next_candidate_cursor: None,
        rejudged_pair: None,
        receipts: Vec::new(),
    }
}

struct RelatePublicationOutput {
    claims_related: usize,
    outcome: RelateOutcome,
    authority_warnings: Vec<crate::relate::settlement::AuthorityWarning>,
    supersessions: Vec<RelateSupersessionRow>,
    conflicts: Vec<RelateConflictRow>,
    rejudged_pair: Option<RejudgedPairRow>,
    strict: bool,
}

fn finish_relate_output(output: RelatePublicationOutput) -> Result<RelateRunOutput, TexoError> {
    let RelatePublicationOutput {
        claims_related,
        outcome,
        authority_warnings,
        supersessions,
        conflicts,
        rejudged_pair,
        strict,
    } = output;
    let (completion, unresolved, held, candidate_pairs, candidate_pair_budget, next_cursor) =
        match outcome {
            RelateOutcome::Complete(complete) => (
                RelateCompletion::Complete,
                Vec::new(),
                complete.held,
                complete.candidate_pairs,
                complete.candidate_pair_budget,
                None,
            ),
            RelateOutcome::Partial(partial) => (
                RelateCompletion::Partial,
                partial.unresolved,
                partial.held,
                partial.candidate_pairs,
                partial.candidate_pair_budget,
                Some(partial.next_candidate_cursor.offset()),
            ),
        };
    let mut warnings = Vec::new();
    if !unresolved.is_empty() {
        warnings.push(
            "semantic settlement is incomplete; unresolved pairs remain authoritative gaps"
                .to_string(),
        );
    }
    if let Some(cursor) = next_cursor {
        warnings.push(format!(
            "candidate page is incomplete; resume deterministically from candidate cursor {cursor}"
        ));
        if strict {
            warnings.push(
                "strict settlement withheld all derived authority until completion".to_string(),
            );
        }
    }
    Ok(RelateRunOutput {
        outcome: completion,
        claims_related,
        supersessions,
        conflicts,
        unresolved,
        held,
        warnings,
        authority_warnings,
        candidate_pairs,
        candidate_pair_budget,
        next_candidate_cursor: next_cursor,
        rejudged_pair,
        receipts: take_receipts()?,
    })
}

#[cfg(test)]
mod tests {
    use super::{campaign_execution, CampaignExecution, CampaignStart};
    use crate::semantics::pipeline::CandidateCursor;

    #[test]
    fn completed_campaign_rejudge_does_not_restart_candidate_paging() {
        assert_eq!(
            campaign_execution(CampaignStart::AlreadyComplete, true),
            CampaignExecution::RejudgeComplete
        );
        assert_eq!(
            campaign_execution(CampaignStart::AlreadyComplete, false),
            CampaignExecution::NoopComplete
        );
        assert_eq!(
            campaign_execution(CampaignStart::Run(CandidateCursor::from_offset(17)), true),
            CampaignExecution::Run(CandidateCursor::from_offset(17))
        );
    }
}
