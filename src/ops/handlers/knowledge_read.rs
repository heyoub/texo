use super::claims::claim_list_rows;
use super::common::{append_json, coverage_for_view, evidence_projection_through, take_receipts};
use super::knowledge::KnowledgeTriangulateOutput;
use super::model::AgentClaimRow;
use super::relate::settlement_is_complete;
use crate::claims::workspace::WorkspaceView;
use crate::code_index::load as load_code_index;
use crate::error::{SnapshotFailureKind, TexoError};
use crate::events::coordinate::{entity_for_claim, scope_for_workspace};
use crate::events::ids::{ClaimId, WorkspaceId};
use crate::events::payloads::{
    ClaimEvidenceLinkedV1, CodeIndexRecordedV1, EvidenceOccurrenceRecordedV1,
    SourceSnapshotRecordedV1, SourceSnapshotRelationV1,
};
use crate::git_source::{CapturedLayer, CapturedSource};
use crate::knowledge::{
    AnalysisQuality, AnswerState, ByteRange, ClaimEvidence, CodeIndexArtifact, CodeOccurrence,
    CoverageGap, CoverageGapKind, EvidenceLinkMethod, EvidenceOccurrence, EvidenceOccurrenceId,
    EvidenceSourceKind, EvidenceStance, KnowledgeCoverage, LineRange, SnapshotRead,
    TriangulationTarget, UncertaintyReason, MAX_EVIDENCE_EXCERPT_BYTES,
};
use crate::ops::env;
use crate::ops::env::ReceiptNote;
use batpak::coordinate::Region;
use batpak::event::EventPayload;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const MAX_TRIANGULATION_CODE_OCCURRENCES: usize = 200;

pub(super) fn triangulate_from_view(
    view: &WorkspaceView,
    snapshot: &SnapshotRead,
    target: TriangulationTarget,
) -> Result<KnowledgeTriangulateOutput, TexoError> {
    validate_triangulation_target(&target)?;
    let projection = evidence_projection_through(view.frontier)?;
    let claim_ids = triangulation_claim_ids(view, &target)?;
    let assertions = claim_list_rows(view, None)?
        .into_iter()
        .filter(|claim| claim_ids.contains(&claim.claim_id))
        .collect::<Vec<_>>();
    let mut evidence = claim_ids
        .iter()
        .flat_map(|claim_id| projection.for_claim(claim_id).iter().cloned())
        .collect::<Vec<_>>();
    evidence.retain(|item| evidence_matches_target(item, &target));
    let mut coverage = coverage_for_view(view, snapshot)?;
    let code = code_evidence_for_target(view.frontier, snapshot, &target)?;
    if let Some(code_coverage) = &code.coverage {
        merge_coverage(&mut coverage, code_coverage);
    }
    if projection.is_incomplete() {
        coverage.gaps.push(CoverageGap {
            path: None,
            kind: CoverageGapKind::AnalysisIncomplete,
        });
    }
    let settlement_complete = settlement_is_complete(view)?;
    let mut uncertainty = BTreeSet::new();
    if snapshot.descriptor.source_snapshot_id.is_none() {
        uncertainty.insert(UncertaintyReason::SourceSnapshotUnavailable);
    }
    if coverage.truncated || !coverage.gaps.is_empty() {
        uncertainty.insert(UncertaintyReason::PartialCoverage);
    }
    if !settlement_complete {
        uncertainty.insert(UncertaintyReason::SettlementIncomplete);
    }
    if matches!(target, TriangulationTarget::Symbol { .. }) && code.unavailable {
        uncertainty.insert(UncertaintyReason::CodeIndexUnavailable);
        if !coverage
            .gaps
            .iter()
            .any(|gap| gap.kind == CoverageGapKind::CodeIndexUnavailable)
        {
            coverage.gaps.push(CoverageGap {
                path: None,
                kind: CoverageGapKind::CodeIndexUnavailable,
            });
        }
    }
    if !assertions.is_empty() && evidence.is_empty() {
        uncertainty.insert(UncertaintyReason::ExactEvidenceUnavailable);
    }
    let answer_state = answer_state_for_rows(&assertions, &evidence, &code.rows);
    Ok(KnowledgeTriangulateOutput {
        target,
        answer_state,
        assertions,
        evidence,
        structural_evidence: code.rows,
        uncertainty: uncertainty.into_iter().collect(),
        coverage,
        settlement_complete,
        snapshot: snapshot.clone(),
    })
}

#[derive(Default)]
pub(super) struct CodeEvidenceLookup {
    rows: Vec<CodeOccurrence>,
    coverage: Option<KnowledgeCoverage>,
    unavailable: bool,
}

#[derive(Default)]
pub(super) struct LoadedCodeArtifact {
    pub(super) artifact: Option<CodeIndexArtifact>,
    pub(super) coverage: Option<KnowledgeCoverage>,
    pub(super) unavailable: bool,
}

pub(super) fn code_evidence_for_target(
    frontier: u64,
    snapshot: &SnapshotRead,
    target: &TriangulationTarget,
) -> Result<CodeEvidenceLookup, TexoError> {
    let Some(source_snapshot_id) = snapshot.descriptor.source_snapshot_id.as_ref() else {
        return Ok(CodeEvidenceLookup {
            unavailable: true,
            ..CodeEvidenceLookup::default()
        });
    };
    let loaded = load_code_artifact_at(frontier, source_snapshot_id)?;
    let Some(artifact) = loaded.artifact else {
        return Ok(CodeEvidenceLookup {
            coverage: loaded.coverage,
            unavailable: loaded.unavailable,
            ..CodeEvidenceLookup::default()
        });
    };
    let mut rows = artifact
        .occurrences
        .into_iter()
        .filter(|occurrence| code_occurrence_matches(occurrence, target))
        .take(MAX_TRIANGULATION_CODE_OCCURRENCES + 1)
        .collect::<Vec<_>>();
    let mut coverage = artifact.coverage;
    if rows.len() > MAX_TRIANGULATION_CODE_OCCURRENCES {
        rows.truncate(MAX_TRIANGULATION_CODE_OCCURRENCES);
        coverage.truncated = true;
        if !coverage
            .gaps
            .iter()
            .any(|gap| gap.path.is_none() && gap.kind == CoverageGapKind::BudgetExceeded)
        {
            coverage.gaps.push(CoverageGap {
                path: None,
                kind: CoverageGapKind::BudgetExceeded,
            });
        }
    }
    Ok(CodeEvidenceLookup {
        rows,
        coverage: Some(coverage),
        unavailable: false,
    })
}

pub(super) fn load_code_artifact_at(
    frontier: u64,
    source_snapshot_id: &crate::knowledge::SourceSnapshotId,
) -> Result<LoadedCodeArtifact, TexoError> {
    let Some(recorded) = latest_code_index(Some(frontier), source_snapshot_id)? else {
        return Ok(LoadedCodeArtifact {
            unavailable: true,
            ..LoadedCodeArtifact::default()
        });
    };
    let artifact = env::with(|op_env| {
        load_code_index(
            &op_env.root,
            &recorded.index_id,
            &recorded.artifact_digest_hex,
        )
    })??;
    if artifact
        .as_ref()
        .is_some_and(|artifact| artifact.snapshot_id != *source_snapshot_id)
    {
        return Err(TexoError::Snapshot {
            kind: SnapshotFailureKind::SourceUnavailable,
            detail: "code-index artifact belongs to a different source snapshot".to_string(),
        });
    }
    Ok(LoadedCodeArtifact {
        unavailable: artifact.is_none(),
        coverage: Some(recorded.coverage),
        artifact,
    })
}

pub(super) fn latest_code_index(
    frontier: Option<u64>,
    snapshot_id: &crate::knowledge::SourceSnapshotId,
) -> Result<Option<CodeIndexRecordedV1>, TexoError> {
    env::with(|op_env| {
        let region = Region::scope(scope_for_workspace(&op_env.workspace_id));
        let mut after = None;
        let mut latest = None;
        'pages: loop {
            let page = op_env.store.query_entries_after(&region, after, 256);
            if page.is_empty() {
                break;
            }
            for entry in &page {
                if frontier.is_some_and(|frontier| entry.global_sequence() > frontier) {
                    break 'pages;
                }
                if entry.event_kind() == <CodeIndexRecordedV1 as EventPayload>::KIND {
                    let raw = op_env.store.read_raw(entry.event_id())?;
                    let payload =
                        batpak::encoding::from_bytes::<CodeIndexRecordedV1>(&raw.event.payload)
                            .map_err(|error| TexoError::Decode {
                                entity: entry.coord().entity().to_string(),
                                detail: error.to_string(),
                            })?;
                    if payload.snapshot_id == *snapshot_id {
                        latest = Some(payload);
                    }
                }
            }
            after = page.last().map(batpak::store::IndexEntry::global_sequence);
        }
        Ok::<_, TexoError>(latest)
    })?
}

pub(super) fn code_occurrence_matches(
    occurrence: &CodeOccurrence,
    target: &TriangulationTarget,
) -> bool {
    match target {
        TriangulationTarget::Claim { .. } => false,
        TriangulationTarget::Path {
            path,
            line_start,
            line_end,
        } => {
            occurrence.path == *path
                && line_start.is_none_or(|start| occurrence.line_range.end >= start)
                && line_end.is_none_or(|end| occurrence.line_range.start <= end)
        }
        TriangulationTarget::Symbol { symbol } => {
            occurrence.symbol == *symbol || occurrence.display_name == *symbol
        }
    }
}

pub(super) fn merge_coverage(target: &mut KnowledgeCoverage, code: &KnowledgeCoverage) {
    if analysis_quality_rank(code.analysis_quality) > analysis_quality_rank(target.analysis_quality)
    {
        target.analysis_quality = code.analysis_quality;
    }
    target.sources_examined = target.sources_examined.max(code.sources_examined);
    target.occurrences = target.occurrences.saturating_add(code.occurrences);
    target.truncated |= code.truncated;
    for gap in &code.gaps {
        if target.gaps.len() >= 256 {
            target.truncated = true;
            break;
        }
        if !target.gaps.contains(gap) {
            target.gaps.push(gap.clone());
        }
    }
}

const fn analysis_quality_rank(quality: AnalysisQuality) -> u8 {
    match quality {
        AnalysisQuality::Precise => 3,
        AnalysisQuality::Syntactic => 2,
        AnalysisQuality::Lexical => 1,
        AnalysisQuality::Unavailable => 0,
    }
}

pub(super) fn validate_triangulation_target(target: &TriangulationTarget) -> Result<(), TexoError> {
    match target {
        TriangulationTarget::Claim { claim_id } if claim_id.is_empty() => Err(TexoError::OpInput {
            op: "texo.knowledge.triangulate".to_string(),
            detail: "claim_id must not be empty".to_string(),
        }),
        TriangulationTarget::Path {
            path,
            line_start,
            line_end,
        } => {
            let safe = !path.is_empty()
                && !Path::new(path).is_absolute()
                && Path::new(path)
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_)));
            let valid_range = match (*line_start, *line_end) {
                (None, None) => true,
                (Some(start), Some(end)) => start > 0 && start <= end,
                _ => false,
            };
            if safe && valid_range {
                Ok(())
            } else {
                Err(TexoError::OpInput {
                    op: "texo.knowledge.triangulate".to_string(),
                    detail: "path must be repository-relative and line bounds must be absent or an ordered one-based pair".to_string(),
                })
            }
        }
        TriangulationTarget::Symbol { symbol } if symbol.is_empty() || symbol.len() > 1024 => {
            Err(TexoError::OpInput {
                op: "texo.knowledge.triangulate".to_string(),
                detail: "symbol must contain between 1 and 1024 bytes".to_string(),
            })
        }
        TriangulationTarget::Claim { .. } | TriangulationTarget::Symbol { .. } => Ok(()),
    }
}

pub(super) fn triangulation_claim_ids(
    view: &WorkspaceView,
    target: &TriangulationTarget,
) -> Result<BTreeSet<String>, TexoError> {
    match target {
        TriangulationTarget::Claim { claim_id } => {
            if view
                .claims
                .iter()
                .any(|claim| claim.card.claim_id == *claim_id)
            {
                Ok(BTreeSet::from([claim_id.clone()]))
            } else {
                Err(TexoError::MissingEntity {
                    entity: entity_for_claim(claim_id),
                })
            }
        }
        TriangulationTarget::Path {
            path,
            line_start,
            line_end,
        } => Ok(view
            .claims
            .iter()
            .filter(|claim| claim.card.source_path == *path)
            .filter(|claim| {
                line_start.is_none_or(|start| claim.card.line_end >= start)
                    && line_end.is_none_or(|end| claim.card.line_start <= end)
            })
            .map(|claim| claim.card.claim_id.clone())
            .collect()),
        TriangulationTarget::Symbol { .. } => Ok(BTreeSet::new()),
    }
}

pub(super) fn evidence_matches_target(
    evidence: &ClaimEvidence,
    target: &TriangulationTarget,
) -> bool {
    match target {
        TriangulationTarget::Claim { .. } => true,
        TriangulationTarget::Path {
            path,
            line_start,
            line_end,
        } => {
            evidence.occurrence.path == *path
                && line_start.is_none_or(|start| evidence.occurrence.line_range.end >= start)
                && line_end.is_none_or(|end| evidence.occurrence.line_range.start <= end)
        }
        TriangulationTarget::Symbol { .. } => false,
    }
}

pub(super) fn answer_state_for_rows(
    assertions: &[AgentClaimRow],
    evidence: &[ClaimEvidence],
    structural_evidence: &[CodeOccurrence],
) -> AnswerState {
    use crate::claims::status::ClaimStatus;
    if evidence
        .iter()
        .any(|item| item.stance == EvidenceStance::Contradicts)
    {
        AnswerState::Contradicted
    } else if assertions
        .iter()
        .any(|claim| claim.status == ClaimStatus::Conflicting)
    {
        AnswerState::Incomparable
    } else if assertions
        .iter()
        .any(|claim| claim.status == ClaimStatus::Superseded)
    {
        AnswerState::Stale
    } else if (!assertions.is_empty()
        && evidence
            .iter()
            .any(|item| item.stance == EvidenceStance::Supports))
        || !structural_evidence.is_empty()
    {
        AnswerState::Supported
    } else {
        AnswerState::Unverified
    }
}

pub(super) fn answer_state_for_claim(
    status: Option<crate::claims::status::ClaimStatus>,
    evidence: &[ClaimEvidence],
) -> AnswerState {
    use crate::claims::status::ClaimStatus;
    match status {
        Some(ClaimStatus::Superseded) => AnswerState::Stale,
        Some(ClaimStatus::Conflicting) => AnswerState::Incomparable,
        Some(ClaimStatus::Current)
            if evidence
                .iter()
                .any(|item| item.stance == EvidenceStance::Contradicts) =>
        {
            AnswerState::Contradicted
        }
        Some(ClaimStatus::Current)
            if evidence
                .iter()
                .any(|item| item.stance == EvidenceStance::Supports) =>
        {
            AnswerState::Supported
        }
        Some(ClaimStatus::Current) | None => AnswerState::Unverified,
    }
}

pub(super) struct EvidencePlan {
    pub(super) rows: Vec<(EvidenceOccurrence, ClaimEvidenceLinkedV1)>,
    pub(super) gaps: Vec<CoverageGap>,
}

pub(super) fn append_knowledge_plan(
    cx: &mut syncbat::Ctx<'_>,
    workspace_id: &WorkspaceId,
    snapshot: &SourceSnapshotRecordedV1,
    rows: &[(EvidenceOccurrence, ClaimEvidenceLinkedV1)],
    relations: &[SourceSnapshotRelationV1],
    observed_at_ms: u64,
) -> Result<Vec<ReceiptNote>, TexoError> {
    append_json(
        "texo.knowledge.index",
        cx,
        <SourceSnapshotRecordedV1 as EventPayload>::KIND,
        snapshot,
    )?;
    for (occurrence, link) in rows {
        append_json(
            "texo.knowledge.index",
            cx,
            <EvidenceOccurrenceRecordedV1 as EventPayload>::KIND,
            &EvidenceOccurrenceRecordedV1 {
                workspace_id: workspace_id.clone(),
                occurrence: occurrence.clone(),
                observed_at_ms,
            },
        )?;
        append_json(
            "texo.knowledge.index",
            cx,
            <ClaimEvidenceLinkedV1 as EventPayload>::KIND,
            link,
        )?;
    }
    for relation in relations {
        append_json(
            "texo.knowledge.index",
            cx,
            <SourceSnapshotRelationV1 as EventPayload>::KIND,
            relation,
        )?;
    }
    take_receipts()
}

pub(super) fn plan_claim_evidence(
    view: &WorkspaceView,
    sources: &[CapturedSource],
    snapshot_id: &crate::knowledge::SourceSnapshotId,
    observed_at_ms: u64,
) -> Result<EvidencePlan, TexoError> {
    let by_path = sources
        .iter()
        .map(|source| (source.path.as_str(), source))
        .collect::<BTreeMap<_, _>>();
    let workspace_id = WorkspaceId::new(view.workspace_id.clone())?;
    let mut rows = Vec::new();
    let mut gaps = Vec::new();
    for claim in &view.claims {
        match plan_claim_evidence_row(
            claim,
            by_path.get(claim.card.source_path.as_str()).copied(),
            snapshot_id,
            &workspace_id,
            observed_at_ms,
        )? {
            PlannedEvidence::Skip => {}
            PlannedEvidence::Gap(gap) => gaps.push(gap),
            PlannedEvidence::Row(row) => rows.push(*row),
        }
    }
    Ok(EvidencePlan { rows, gaps })
}

enum PlannedEvidence {
    Skip,
    Gap(CoverageGap),
    Row(Box<(EvidenceOccurrence, ClaimEvidenceLinkedV1)>),
}

fn plan_claim_evidence_row(
    claim: &crate::claims::workspace::ClaimView,
    source: Option<&CapturedSource>,
    snapshot_id: &crate::knowledge::SourceSnapshotId,
    workspace_id: &WorkspaceId,
    observed_at_ms: u64,
) -> Result<PlannedEvidence, TexoError> {
    let Some(source) = source else {
        return Ok(PlannedEvidence::Skip);
    };
    let source_digest_hex = crate::events::ids::blake3_bytes_hex(&source.bytes);
    let captured_source_id = crate::events::ids::source_id_from_hash(&source_digest_hex)?;
    if captured_source_id.as_str() != claim.card.source_id {
        return Ok(PlannedEvidence::Gap(CoverageGap {
            path: Some(source.path.clone()),
            kind: CoverageGapKind::AnalysisIncomplete,
        }));
    }
    let Some((start, end)) =
        line_byte_range(&source.bytes, claim.card.line_start, claim.card.line_end)
    else {
        return Ok(PlannedEvidence::Gap(CoverageGap {
            path: Some(source.path.clone()),
            kind: CoverageGapKind::AnalysisIncomplete,
        }));
    };
    let excerpt_bytes = &source.bytes[start..end];
    let Ok(excerpt) = std::str::from_utf8(excerpt_bytes) else {
        return Ok(PlannedEvidence::Gap(CoverageGap {
            path: Some(source.path.clone()),
            kind: CoverageGapKind::UnsupportedEncoding,
        }));
    };
    if excerpt.len() > MAX_EVIDENCE_EXCERPT_BYTES {
        return Ok(PlannedEvidence::Gap(CoverageGap {
            path: Some(source.path.clone()),
            kind: CoverageGapKind::SourceTooLarge,
        }));
    }
    let material = format!(
        "texo.evidence.occurrence.v1\u{1f}{snapshot_id}\u{1f}{}\u{1f}{start}\u{1f}{end}\u{1f}{}",
        source.path, claim.card.claim_id
    );
    let occurrence_id = EvidenceOccurrenceId::derive(&material);
    let occurrence = EvidenceOccurrence {
        occurrence_id: occurrence_id.clone(),
        snapshot_id: snapshot_id.clone(),
        source_kind: match source.layer {
            CapturedLayer::Committed => EvidenceSourceKind::GitBlob,
            CapturedLayer::Worktree => EvidenceSourceKind::WorktreeOverlay,
        },
        path: source.path.clone(),
        byte_range: ByteRange::new(
            u64::try_from(start).unwrap_or(u64::MAX),
            u64::try_from(end).unwrap_or(u64::MAX),
        )
        .map_err(|error| TexoError::Source {
            path: source.path.clone(),
            detail: error.to_string(),
        })?,
        line_range: LineRange::new(claim.card.line_start, claim.card.line_end).map_err(
            |error| TexoError::Source {
                path: source.path.clone(),
                detail: error.to_string(),
            },
        )?,
        git_blob: source.blob_id.clone(),
        source_digest_hex,
        excerpt: excerpt.to_string(),
        analyzer_fingerprint: format!(
            "{}:{}:{}",
            claim.card.extractor_kind, claim.card.extractor_model, claim.card.prompt_version
        ),
        analysis_quality: AnalysisQuality::Syntactic,
    };
    occurrence.validate().map_err(|error| TexoError::Source {
        path: source.path.clone(),
        detail: error.to_string(),
    })?;
    let link = ClaimEvidenceLinkedV1 {
        workspace_id: workspace_id.clone(),
        claim_id: ClaimId::try_from(claim.card.claim_id.as_str())?,
        occurrence_id,
        stance: EvidenceStance::Supports,
        method: EvidenceLinkMethod::Deterministic,
        observed_at_ms,
    };
    Ok(PlannedEvidence::Row(Box::new((occurrence, link))))
}

pub(super) fn line_byte_range(
    bytes: &[u8],
    start_line: u32,
    end_line: u32,
) -> Option<(usize, usize)> {
    if start_line == 0 || end_line < start_line {
        return None;
    }
    let mut line = 1_u32;
    let mut line_start = 0_usize;
    let mut range_start = None;
    for offset in 0..=bytes.len() {
        let boundary = offset == bytes.len() || bytes.get(offset) == Some(&b'\n');
        if !boundary {
            continue;
        }
        if line == start_line {
            range_start = Some(line_start);
        }
        if line == end_line {
            return range_start.map(|start| (start, offset));
        }
        line = line.saturating_add(1);
        line_start = offset.saturating_add(1);
    }
    None
}
