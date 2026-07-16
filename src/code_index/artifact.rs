use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::error::TexoError;
use crate::events::ids::blake3_bytes_hex;
use crate::git_source::GitCapture;
use crate::knowledge::{
    AnalysisQuality, CodeIndexArtifact, CodeIndexFormat, CodeIndexId, CodeOccurrence, CoverageGap,
    CoverageGapKind, KnowledgeCoverage,
};

use super::analysis::analyze_fallbacks;
use super::scip_import::import_scip;
use super::util::{code_occurrence_order, source_error};
use super::{CodeIndexLimits, PreparedCodeIndex, ARTIFACT_SCHEMA};

const MAX_GAPS: usize = 256;

/// Build a deterministic code index for one frozen Git capture.
///
/// SCIP occurrences are preferred. Sources absent from the imported SCIP
/// index fall back to the built-in Rust grammar or bounded lexical discovery.
///
/// # Errors
/// Returns a typed source error for malformed SCIP, invalid source ranges, or
/// artifact serialization failure.
pub fn build(
    capture: &GitCapture,
    scip_bytes: Option<&[u8]>,
    limits: CodeIndexLimits,
) -> Result<PreparedCodeIndex, TexoError> {
    let source_map = capture
        .sources
        .iter()
        .map(|source| (source.path.as_str(), source))
        .collect::<BTreeMap<_, _>>();
    let mut builder = ArtifactBuilder::new(limits, capture.coverage.clone());
    let mut indexed_paths = BTreeSet::new();
    let mut analyzer_parts = Vec::new();
    if let Some(bytes) = scip_bytes {
        let before = builder.occurrences.len();
        let analyzer = import_scip(bytes, &source_map, &mut builder, &mut indexed_paths)?;
        analyzer_parts.push(analyzer);
        // Only claim compiler-precise coverage when SCIP actually contributed
        // occurrences. An empty index, all-missing documents, or all-skipped
        // ranges must not promote lexical/syntactic rows to `precise`.
        if builder.occurrences.len() > before {
            builder.format = CodeIndexFormat::Scip;
            builder.quality = AnalysisQuality::Precise;
        }
    }
    let fallback = analyze_fallbacks(
        &capture.sources,
        &indexed_paths,
        &mut builder,
        limits.analysis_budget,
    )?;
    if !fallback.is_empty() {
        analyzer_parts.push(fallback);
    }
    if analyzer_parts.is_empty() {
        analyzer_parts.push("texo-lexical:v2".to_string());
    }
    let analyzer_fingerprint = analyzer_parts.join("+");
    let mut occurrences = builder.occurrences;
    occurrences.sort_by(code_occurrence_order);
    occurrences.dedup();
    let occurrence_material = batpak::encoding::to_bytes(&occurrences)
        .map_err(|error| source_error(Path::new(".texo/cache/code-index"), &error.to_string()))?;
    // Derive identity from the normalized occurrences actually persisted, not the
    // raw SCIP bytes: two builds of the same snapshot+SCIP under different limits
    // truncate differently, and the id must track what lands on disk so the cache
    // never serves a digest-mismatched or silently truncated artifact.
    let raw_digest = blake3_bytes_hex(&occurrence_material);
    let index_id = CodeIndexId::derive(&format!(
        "{ARTIFACT_SCHEMA}\u{1f}{}\u{1f}{raw_digest}\u{1f}{analyzer_fingerprint}",
        capture.snapshot_id
    ));
    let coverage = KnowledgeCoverage {
        analysis_quality: builder.quality,
        sources_examined: builder.sources_examined,
        occurrences: u64::try_from(occurrences.len()).unwrap_or(u64::MAX),
        truncated: builder.truncated,
        gaps: builder.gaps,
    };
    let artifact = CodeIndexArtifact {
        schema: ARTIFACT_SCHEMA.to_string(),
        snapshot_id: capture.snapshot_id.clone(),
        index_id,
        format: builder.format,
        analyzer_fingerprint,
        occurrences,
        coverage,
    };
    let bytes = batpak::encoding::to_bytes(&artifact)
        .map_err(|error| source_error(Path::new(".texo/cache/code-index"), &error.to_string()))?;
    let artifact_digest_hex = blake3_bytes_hex(&bytes);
    Ok(PreparedCodeIndex {
        artifact,
        artifact_digest_hex,
        bytes,
    })
}

pub(super) struct ArtifactBuilder {
    pub(super) limits: CodeIndexLimits,
    pub(super) occurrences: Vec<CodeOccurrence>,
    pub(super) sources_examined: u64,
    pub(super) truncated: bool,
    pub(super) gaps: Vec<CoverageGap>,
    pub(super) format: CodeIndexFormat,
    pub(super) quality: AnalysisQuality,
}

impl ArtifactBuilder {
    pub(super) fn new(limits: CodeIndexLimits, source_coverage: KnowledgeCoverage) -> Self {
        Self {
            limits,
            occurrences: Vec::new(),
            sources_examined: 0,
            truncated: source_coverage.truncated,
            gaps: source_coverage.gaps,
            format: CodeIndexFormat::Lexical,
            quality: AnalysisQuality::Lexical,
        }
    }

    pub(super) fn push(&mut self, occurrence: CodeOccurrence) -> bool {
        if self.occurrences.len() >= self.limits.max_occurrences {
            self.truncated = true;
            self.gap(None, CoverageGapKind::BudgetExceeded);
            return false;
        }
        self.occurrences.push(occurrence);
        true
    }

    pub(super) fn gap(&mut self, path: Option<String>, kind: CoverageGapKind) {
        if self.gaps.len() < MAX_GAPS {
            let gap = CoverageGap { path, kind };
            if !self.gaps.contains(&gap) {
                self.gaps.push(gap);
            }
        } else {
            self.truncated = true;
        }
    }
}
