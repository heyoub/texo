//! Bounded proposal-only reconciliation between semantic claims and code.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use crate::events::ids::ClaimId;
use crate::knowledge::{
    AnalysisQuality, CodeIndexArtifact, CodeOccurrence, EvidenceOccurrence, EvidenceOccurrenceId,
    EvidenceSourceKind, EvidenceStance, SourceSnapshotId,
};
use crate::relate::settlement::{PairFailureView, RelationFailureClass};
use crate::semantics::{ClaimRelater, ClaimRelation, RelationVerdict};

/// Versioned deterministic policy and prompt context.
pub const POLICY_VERSION: &str = "evidence-reconcile-v1";

/// Bounds on candidate generation before any paid proposal call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileLimits {
    /// Maximum candidates retained for one claim.
    pub per_claim: usize,
    /// Maximum candidates retained for the complete operation.
    pub total: usize,
    /// Minimum accepted model score in parts per million.
    pub min_score_ppm: u32,
}

impl Default for ReconcileLimits {
    fn default() -> Self {
        Self {
            per_claim: 4,
            total: 256,
            min_score_ppm: 700_000,
        }
    }
}

/// Minimal semantic claim view consumed by candidate generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileClaim {
    /// Durable claim identity.
    pub claim_id: ClaimId,
    /// Exact assertion text sent to the proposal model.
    pub text: String,
    /// Optional deterministic subject hint.
    pub subject_hint: String,
    /// Optional deterministic predicate hint.
    pub predicate_hint: String,
    /// Optional deterministic object hint.
    pub object_hint: String,
}

/// One bounded claim/code pair eligible for a cached model proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileCandidate {
    /// Claim receiving evidence if policy accepts the proposal.
    pub claim_id: ClaimId,
    /// Exact semantic assertion supplied as the proposal's first assertion.
    pub claim_text: String,
    /// Exact durable occurrence constructed from the disposable code index.
    pub occurrence: EvidenceOccurrence,
    /// Role-labelled code text supplied as the proposal's second assertion.
    pub code_prompt: String,
    rank: usize,
}

/// One cached-or-live proposal ready for deterministic policy evaluation.
#[derive(Debug, Clone)]
pub struct CandidateProposal {
    /// Original bounded candidate.
    pub candidate: ReconcileCandidate,
    /// Non-authoritative model output.
    pub verdict: RelationVerdict,
}

/// One candidate whose proposal remains unavailable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedCandidate {
    /// Original bounded candidate.
    pub candidate: ReconcileCandidate,
    /// Sanitized closed failure class.
    pub failure: PairFailureView,
}

/// Deterministically reassembled proposal acquisition result.
#[derive(Debug, Default)]
pub struct ProposalBatch {
    /// Proposals in original candidate order.
    pub proposals: Vec<CandidateProposal>,
    /// Failures in original candidate order.
    pub unresolved: Vec<UnresolvedCandidate>,
}

/// Result of deterministic bounded candidate generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidatePlan {
    /// Stable candidates ordered by claim then descending lexical rank.
    pub candidates: Vec<ReconcileCandidate>,
    /// True when a configured candidate bound omitted additional pairs.
    pub truncated: bool,
}

/// Generate bounded candidates without a model, parser, or mutable source read.
#[must_use]
pub fn plan_candidates(
    claims: &[ReconcileClaim],
    artifact: &CodeIndexArtifact,
    limits: ReconcileLimits,
) -> CandidatePlan {
    let mut candidates = Vec::new();
    let mut truncated = false;
    for claim in claims {
        let claim_tokens = claim_tokens(claim);
        let mut ranked = artifact
            .occurrences
            .iter()
            .filter(|occurrence| is_code_path(&occurrence.path))
            .filter_map(|occurrence| {
                let rank = candidate_rank(&claim_tokens, occurrence);
                (rank > 0).then(|| candidate(claim, &artifact.snapshot_id, occurrence, rank))
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right.rank.cmp(&left.rank).then_with(|| {
                left.occurrence
                    .occurrence_id
                    .as_str()
                    .cmp(right.occurrence.occurrence_id.as_str())
            })
        });
        if ranked.len() > limits.per_claim {
            ranked.truncate(limits.per_claim);
            truncated = true;
        }
        candidates.extend(ranked);
        if candidates.len() > limits.total {
            candidates.truncate(limits.total);
            truncated = true;
            break;
        }
    }
    CandidatePlan {
        candidates,
        truncated,
    }
}

fn is_code_path(path: &str) -> bool {
    let path = std::path::Path::new(path);
    // Share the capture/index basename scope so reconcile never drops evidence
    // for an extensionless config file that indexing already accepted.
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(crate::git_source::is_wellknown_source_basename)
    {
        return true;
    }
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "rs" | "py"
                    | "js"
                    | "jsx"
                    | "ts"
                    | "tsx"
                    | "go"
                    | "java"
                    | "kt"
                    | "kts"
                    | "rb"
                    | "php"
                    | "c"
                    | "cc"
                    | "cpp"
                    | "cxx"
                    | "h"
                    | "hpp"
                    | "cs"
                    | "swift"
                    | "scala"
                    | "sh"
                    | "bash"
                    | "zsh"
                    | "fish"
                    | "sql"
                    | "toml"
                    | "yaml"
                    | "yml"
                    | "json"
                    | "proto"
                    | "tf"
                    | "hcl"
                    | "ex"
                    | "exs"
                    | "erl"
                    | "hrl"
                    | "clj"
                    | "cljs"
                    | "vue"
                    | "svelte"
            )
        })
}

/// Accept or reject one cached model proposal under closed deterministic policy.
#[must_use]
pub fn accept_proposal(
    verdict: RelationVerdict,
    min_score_ppm: u32,
) -> Option<(EvidenceStance, u32)> {
    let score_ppm = score_to_ppm(verdict.score);
    if score_ppm < min_score_ppm {
        return None;
    }
    let stance = match verdict.relation {
        ClaimRelation::Duplicate => EvidenceStance::Supports,
        ClaimRelation::Supersedes | ClaimRelation::Conflict => EvidenceStance::Contradicts,
        ClaimRelation::Unrelated => return None,
    };
    Some((stance, score_ppm))
}

/// Acquire proposals concurrently and reassemble them in candidate order.
///
/// The model remains proposal-only: this function has no journal or policy
/// access. A global budget cuts off not-yet-started work, while failures remain
/// isolated to their candidate.
///
/// # Errors
/// Returns an I/O error when a named worker thread cannot be created.
pub fn acquire_proposals(
    candidates: &[ReconcileCandidate],
    relater: &(dyn ClaimRelater + Sync),
    budget: Duration,
    concurrency: usize,
) -> Result<ProposalBatch, std::io::Error> {
    if candidates.is_empty() {
        return Ok(ProposalBatch::default());
    }
    let workers = concurrency.clamp(1, 16).min(candidates.len());
    let started = Instant::now();
    let (job_tx, job_rx) = flume::bounded::<usize>(workers);
    let (result_tx, result_rx) = flume::unbounded();
    std::thread::scope(|scope| -> Result<(), std::io::Error> {
        for worker in 0..workers {
            let job_rx = job_rx.clone();
            let result_tx = result_tx.clone();
            std::thread::Builder::new()
                .name(format!("texo-reconcile-{worker}"))
                .spawn_scoped(scope, move || {
                    while let Ok(index) = job_rx.recv() {
                        let outcome = if started.elapsed() >= budget {
                            Err(budget_failure())
                        } else {
                            relater
                                .relate(
                                    &candidates[index].claim_text,
                                    &candidates[index].code_prompt,
                                )
                                .map_err(|error| {
                                    crate::semantics::pipeline::classify_pair_failure(&error)
                                })
                        };
                        if result_tx.send((index, outcome)).is_err() {
                            return;
                        }
                    }
                })?;
        }
        drop(job_rx);
        drop(result_tx);
        for index in 0..candidates.len() {
            if job_tx.send(index).is_err() {
                break;
            }
        }
        drop(job_tx);
        Ok(())
    })?;
    let mut outcomes = vec![None; candidates.len()];
    for (index, outcome) in result_rx {
        outcomes[index] = Some(outcome);
    }
    let mut batch = ProposalBatch::default();
    for (candidate, outcome) in candidates.iter().cloned().zip(outcomes) {
        match outcome.unwrap_or(Err(budget_failure())) {
            Ok(verdict) => batch
                .proposals
                .push(CandidateProposal { candidate, verdict }),
            Err(failure) => batch
                .unresolved
                .push(UnresolvedCandidate { candidate, failure }),
        }
    }
    Ok(batch)
}

fn budget_failure() -> PairFailureView {
    PairFailureView {
        class: RelationFailureClass::BudgetExhausted,
        endpoint: None,
        status: None,
        attempts: 0,
    }
}

/// Typed operation input for semantic evidence reconciliation.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct KnowledgeReconcileInput {
    pub(crate) observed_at_ms: u64,
    #[serde(default)]
    max_per_claim: Option<usize>,
    #[serde(default)]
    max_candidates: Option<usize>,
    #[serde(default)]
    min_score_ppm: Option<u32>,
    #[serde(default)]
    budget_secs: Option<u64>,
    #[serde(default)]
    concurrency: Option<usize>,
}

impl KnowledgeReconcileInput {
    /// Validate all explicit resource bounds.
    pub(crate) fn validated(
        &self,
    ) -> Result<(ReconcileLimits, u64, usize), crate::error::TexoError> {
        let defaults = ReconcileLimits::default();
        let limits = ReconcileLimits {
            per_claim: self.max_per_claim.unwrap_or(defaults.per_claim),
            total: self.max_candidates.unwrap_or(defaults.total),
            min_score_ppm: self.min_score_ppm.unwrap_or(defaults.min_score_ppm),
        };
        let budget_secs = self.budget_secs.unwrap_or(900);
        let concurrency = self.concurrency.unwrap_or(4);
        if !(1..=16).contains(&limits.per_claim)
            || !(1..=1_024).contains(&limits.total)
            || limits.min_score_ppm > 1_000_000
            || !(1..=3_600).contains(&budget_secs)
            || !(1..=16).contains(&concurrency)
        {
            return Err(crate::error::TexoError::OpInput {
                op: "texo.knowledge.reconcile".to_string(),
                detail: "reconcile limits require 1..=16 candidates per claim, 1..=1024 total, score <=1000000 ppm, budget 1..=3600 seconds, and concurrency 1..=16".to_string(),
            });
        }
        Ok((limits, budget_secs, concurrency))
    }
}

/// Complete or explicitly partial reconciliation state.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReconcileCompletion {
    Complete,
    Partial,
}

/// Stable operation output for semantic evidence reconciliation.
#[derive(Debug, serde::Serialize)]
pub(crate) struct KnowledgeReconcileOutput {
    pub(crate) outcome: ReconcileCompletion,
    pub(crate) snapshot_id: SourceSnapshotId,
    pub(crate) candidates_considered: usize,
    pub(crate) already_linked: usize,
    pub(crate) accepted: Vec<ReconcileAcceptedRow>,
    pub(crate) rejected: usize,
    pub(crate) unresolved: Vec<ReconcileUnresolvedRow>,
    pub(crate) coverage: crate::knowledge::KnowledgeCoverage,
    pub(crate) receipts: Vec<crate::ops::env::ReceiptNote>,
}

/// One durable policy-accepted link reported to the caller.
#[derive(Debug, serde::Serialize)]
pub(crate) struct ReconcileAcceptedRow {
    pub(crate) claim_id: String,
    pub(crate) occurrence_id: String,
    pub(crate) stance: EvidenceStance,
    pub(crate) score_ppm: u32,
    pub(crate) code_ref: String,
    pub(crate) cache_key_hex: String,
}

/// One candidate without an available proposal.
#[derive(Debug, serde::Serialize)]
pub(crate) struct ReconcileUnresolvedRow {
    pub(crate) claim_id: String,
    pub(crate) occurrence_id: String,
    pub(crate) code_ref: String,
    pub(crate) failure: PairFailureView,
}

/// Cached proposal plus its stable cache identity.
pub(crate) struct CachedCandidateProposal {
    pub(crate) proposal: CandidateProposal,
    pub(crate) cache_key_hex: String,
}

/// Backend acquisition result before deterministic policy evaluation.
pub(crate) struct ReconcileBackendOutput {
    pub(crate) proposals: Vec<CachedCandidateProposal>,
    pub(crate) unresolved: Vec<UnresolvedCandidate>,
    pub(crate) judge_fingerprint: String,
}

/// Convert current workspace claims into the minimal reconciliation view.
pub(crate) fn claims_from_view(
    view: &crate::claims::workspace::WorkspaceView,
) -> Result<Vec<ReconcileClaim>, crate::error::TexoError> {
    view.claims
        .iter()
        .filter(|claim| claim.status == crate::claims::status::ClaimStatus::Current)
        .map(|claim| {
            Ok(ReconcileClaim {
                claim_id: ClaimId::try_from(claim.card.claim_id.as_str())?,
                text: claim.card.text.clone(),
                subject_hint: claim.card.subject_hint.clone().unwrap_or_default(),
                predicate_hint: claim.card.predicate_hint.clone().unwrap_or_default(),
                object_hint: claim.card.object_hint.clone().unwrap_or_default(),
            })
        })
        .collect()
}

/// Render one sanitized unresolved row.
pub(crate) fn unresolved_row(unresolved: &UnresolvedCandidate) -> ReconcileUnresolvedRow {
    ReconcileUnresolvedRow {
        claim_id: unresolved.candidate.claim_id.to_string(),
        occurrence_id: unresolved.candidate.occurrence.occurrence_id.to_string(),
        code_ref: format!(
            "{}:{}",
            unresolved.candidate.occurrence.path, unresolved.candidate.occurrence.line_range.start
        ),
        failure: unresolved.failure.clone(),
    }
}

/// Compose the configured record-once relation backend for evidence proposals.
#[cfg(feature = "openrouter")]
pub(crate) fn evaluate_with_backends(
    root: &std::path::Path,
    gateway: Option<&crate::gateway::GatewayConfig>,
    candidates: &[ReconcileCandidate],
    budget: Duration,
    concurrency: usize,
) -> Result<ReconcileBackendOutput, crate::error::TexoError> {
    use crate::extract::cache::CachingRelater;
    use crate::semantics::openrouter::OpenRouterRelater;

    if candidates.is_empty() {
        return Ok(empty_backend_output());
    }
    let cache_dir = std::env::var_os("TEXO_RECONCILE_CACHE").map_or_else(
        || root.join(".texo/reconcile-cache"),
        std::path::PathBuf::from,
    );
    let relater = CachingRelater::new(
        OpenRouterRelater::new(None, gateway).map_err(semantic_error)?,
        cache_dir,
    );
    let judge_fingerprint = relater.fingerprint();
    let batch = acquire_proposals(candidates, &relater, budget, concurrency)?;
    let proposals = batch
        .proposals
        .into_iter()
        .map(|proposal| CachedCandidateProposal {
            cache_key_hex: relater.cache_key(
                &proposal.candidate.claim_text,
                &proposal.candidate.code_prompt,
            ),
            proposal,
        })
        .collect();
    Ok(ReconcileBackendOutput {
        proposals,
        unresolved: batch.unresolved,
        judge_fingerprint,
    })
}

#[cfg(not(feature = "openrouter"))]
pub(crate) fn evaluate_with_backends(
    _root: &std::path::Path,
    _gateway: Option<&crate::gateway::GatewayConfig>,
    candidates: &[ReconcileCandidate],
    _budget: Duration,
    _concurrency: usize,
) -> Result<ReconcileBackendOutput, crate::error::TexoError> {
    if candidates.is_empty() {
        Ok(empty_backend_output())
    } else {
        Err(crate::error::TexoError::Semantics {
            backend: "openrouter".to_string(),
            detail: "openrouter feature is disabled".to_string(),
        })
    }
}

fn empty_backend_output() -> ReconcileBackendOutput {
    ReconcileBackendOutput {
        proposals: Vec::new(),
        unresolved: Vec::new(),
        judge_fingerprint: String::new(),
    }
}

#[cfg(feature = "openrouter")]
fn semantic_error(
    error: impl std::error::Error + Send + Sync + 'static,
) -> crate::error::TexoError {
    crate::error::TexoError::Semantics {
        backend: "openrouter".to_string(),
        detail: crate::error::error_chain(&error),
    }
}

fn candidate(
    claim: &ReconcileClaim,
    snapshot_id: &SourceSnapshotId,
    code: &CodeOccurrence,
    rank: usize,
) -> ReconcileCandidate {
    let occurrence_id = EvidenceOccurrenceId::derive(&format!(
        "texo.code.evidence.v1\u{1f}{snapshot_id}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        code.symbol,
        code.path,
        code.context_byte_range.start,
        code.context_byte_range.end,
        code.source_digest_hex,
    ));
    let occurrence = EvidenceOccurrence {
        occurrence_id,
        snapshot_id: snapshot_id.clone(),
        source_kind: match code.analysis_quality {
            AnalysisQuality::Precise => EvidenceSourceKind::Scip,
            AnalysisQuality::Syntactic => EvidenceSourceKind::Syntax,
            AnalysisQuality::Lexical | AnalysisQuality::Unavailable => EvidenceSourceKind::Lexical,
        },
        path: code.path.clone(),
        byte_range: code.context_byte_range,
        line_range: code.context_line_range,
        git_blob: None,
        source_digest_hex: code.source_digest_hex.clone(),
        excerpt: code.context.clone(),
        analyzer_fingerprint: code.analyzer_fingerprint.clone(),
        analysis_quality: code.analysis_quality,
    };
    let code_prompt = format!(
        "{POLICY_VERSION}\nCode evidence for symbol `{}` at {}:{}:\n{}",
        code.symbol, code.path, code.context_line_range.start, code.context
    );
    ReconcileCandidate {
        claim_id: claim.claim_id.clone(),
        claim_text: claim.text.clone(),
        occurrence,
        code_prompt,
        rank,
    }
}

fn claim_tokens(claim: &ReconcileClaim) -> BTreeSet<String> {
    tokens(&format!(
        "{} {} {} {}",
        claim.text, claim.subject_hint, claim.predicate_hint, claim.object_hint
    ))
}

fn candidate_rank(claim_tokens: &BTreeSet<String>, code: &CodeOccurrence) -> usize {
    let display = tokens(&format!("{} {}", code.symbol, code.display_name));
    let context = tokens(&format!("{} {}", code.path, code.context));
    let names = claim_tokens.intersection(&display).count();
    let context = claim_tokens.intersection(&context).count();
    names.saturating_mul(4).saturating_add(context)
}

fn tokens(text: &str) -> BTreeSet<String> {
    let mut normalized = String::with_capacity(text.len());
    let mut previous_lower = false;
    for ch in text.chars() {
        if ch.is_ascii_uppercase() && previous_lower {
            normalized.push(' ');
        }
        if ch.is_alphanumeric() {
            normalized.extend(ch.to_lowercase());
            previous_lower = ch.is_lowercase();
        } else {
            normalized.push(' ');
            previous_lower = false;
        }
    }
    normalized
        .split_whitespace()
        .filter(|token| token.len() >= 3 && !STOP_WORDS.contains(token))
        .map(ToOwned::to_owned)
        .collect()
}

const STOP_WORDS: &[&str] = &[
    "and", "are", "for", "from", "into", "not", "our", "the", "this", "that", "use", "uses", "with",
];

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "score is clamped to the closed 0..=1 interval before ppm conversion"
)]
fn score_to_ppm(score: f32) -> u32 {
    (score.clamp(0.0, 1.0) * 1_000_000.0).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{
        ByteRange, CodeIndexFormat, CodeIndexId, CodeOccurrenceRole, KnowledgeCoverage, LineRange,
    };

    fn artifact() -> CodeIndexArtifact {
        let occurrence = CodeOccurrence {
            symbol: "local crate MAX_RETRIES".to_string(),
            display_name: "MAX_RETRIES".to_string(),
            roles: vec![CodeOccurrenceRole::Definition],
            path: "src/config.rs".to_string(),
            byte_range: ByteRange::new(0, 28).expect("range"),
            line_range: LineRange::new(1, 1).expect("line"),
            source_digest_hex: "a".repeat(64),
            excerpt: "const MAX_RETRIES: usize = 4;".to_string(),
            context: "const MAX_RETRIES: usize = 4;".to_string(),
            context_byte_range: ByteRange::new(0, 28).expect("range"),
            context_line_range: LineRange::new(1, 1).expect("line"),
            analyzer_fingerprint: "tree-sitter-rust:test".to_string(),
            analysis_quality: AnalysisQuality::Syntactic,
        };
        let prose = CodeOccurrence {
            symbol: "retries".to_string(),
            display_name: "retries".to_string(),
            roles: vec![CodeOccurrenceRole::Reference],
            path: "docs/retries.md".to_string(),
            byte_range: ByteRange::new(0, 7).expect("range"),
            line_range: LineRange::new(1, 1).expect("line"),
            source_digest_hex: "b".repeat(64),
            excerpt: "retries".to_string(),
            context: "retries".to_string(),
            context_byte_range: ByteRange::new(0, 7).expect("range"),
            context_line_range: LineRange::new(1, 1).expect("line"),
            analyzer_fingerprint: "texo-lexical:v2".to_string(),
            analysis_quality: AnalysisQuality::Lexical,
        };
        CodeIndexArtifact {
            schema: "texo.code-index.v1".to_string(),
            snapshot_id: SourceSnapshotId::derive("snapshot"),
            index_id: CodeIndexId::derive("index"),
            format: CodeIndexFormat::Syntax,
            analyzer_fingerprint: "tree-sitter-rust:test".to_string(),
            occurrences: vec![prose, occurrence],
            coverage: KnowledgeCoverage {
                analysis_quality: AnalysisQuality::Syntactic,
                sources_examined: 1,
                occurrences: 2,
                truncated: false,
                gaps: Vec::new(),
            },
        }
    }

    #[test]
    fn candidate_generation_is_bounded_stable_and_exact() {
        let claim = ReconcileClaim {
            claim_id: ClaimId::try_from("claim_aaaaaaaaaaaa").expect("claim"),
            text: "Retries are limited to four attempts".to_string(),
            subject_hint: "retries".to_string(),
            predicate_hint: "limited".to_string(),
            object_hint: "four".to_string(),
        };
        let first = plan_candidates(
            std::slice::from_ref(&claim),
            &artifact(),
            ReconcileLimits::default(),
        );
        let second = plan_candidates(&[claim], &artifact(), ReconcileLimits::default());

        assert_eq!(first, second);
        assert_eq!(first.candidates.len(), 1);
        assert_eq!(first.candidates[0].occurrence.path, "src/config.rs");
        assert_eq!(
            first.candidates[0].occurrence.excerpt,
            "const MAX_RETRIES: usize = 4;"
        );
        assert_eq!(
            first.candidates[0].occurrence.source_kind,
            EvidenceSourceKind::Syntax
        );
    }

    #[test]
    fn policy_accepts_only_confident_actionable_proposals() {
        assert_eq!(
            accept_proposal(
                RelationVerdict {
                    relation: ClaimRelation::Duplicate,
                    score: 0.9,
                },
                700_000,
            ),
            Some((EvidenceStance::Supports, 900_000))
        );
        assert_eq!(
            accept_proposal(
                RelationVerdict {
                    relation: ClaimRelation::Conflict,
                    score: 0.8,
                },
                700_000,
            ),
            Some((EvidenceStance::Contradicts, 800_000))
        );
        assert_eq!(
            accept_proposal(
                RelationVerdict {
                    relation: ClaimRelation::Unrelated,
                    score: 1.0,
                },
                700_000,
            ),
            None
        );
    }

    struct CountingRelater {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl ClaimRelater for CountingRelater {
        fn relate(
            &self,
            _older: &str,
            _newer: &str,
        ) -> Result<RelationVerdict, crate::semantics::SemanticsError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(RelationVerdict {
                relation: ClaimRelation::Duplicate,
                score: 0.9,
            })
        }

        fn fingerprint(&self) -> String {
            "counting".to_string()
        }
    }

    #[test]
    fn proposal_fanout_reassembles_in_candidate_order_and_budget_fails_closed() {
        let claims = [
            ReconcileClaim {
                claim_id: ClaimId::try_from("claim_aaaaaaaaaaaa").expect("claim"),
                text: "Retries are limited to four attempts".to_string(),
                subject_hint: "retries".to_string(),
                predicate_hint: String::new(),
                object_hint: String::new(),
            },
            ReconcileClaim {
                claim_id: ClaimId::try_from("claim_bbbbbbbbbbbb").expect("claim"),
                text: "Retries use a maximum".to_string(),
                subject_hint: "retries".to_string(),
                predicate_hint: String::new(),
                object_hint: String::new(),
            },
        ];
        let plan = plan_candidates(&claims, &artifact(), ReconcileLimits::default());
        let relater = CountingRelater {
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let batch =
            acquire_proposals(&plan.candidates, &relater, Duration::MAX, 4).expect("workers");
        assert_eq!(batch.proposals.len(), plan.candidates.len());
        assert_eq!(batch.proposals[0].candidate.claim_id, claims[0].claim_id);
        assert_eq!(
            relater.calls.load(std::sync::atomic::Ordering::SeqCst),
            plan.candidates.len()
        );

        let blocked =
            acquire_proposals(&plan.candidates, &relater, Duration::ZERO, 4).expect("workers");
        assert!(blocked.proposals.is_empty());
        assert_eq!(blocked.unresolved.len(), plan.candidates.len());
        assert!(blocked
            .unresolved
            .iter()
            .all(|row| row.failure.class == RelationFailureClass::BudgetExhausted));
    }
}
