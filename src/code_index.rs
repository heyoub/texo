//! Bounded SCIP import and built-in code-intelligence fallbacks.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::ops::ControlFlow;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use protobuf::Message;

use crate::error::TexoError;
use crate::events::ids::blake3_bytes_hex;
use crate::git_source::{CapturedSource, GitCapture};
use crate::knowledge::{
    AnalysisQuality, ByteRange, CodeIndexArtifact, CodeIndexFormat, CodeIndexId, CodeOccurrence,
    CodeOccurrenceRole, CoverageGap, CoverageGapKind, KnowledgeCoverage, LineRange,
    MAX_EVIDENCE_EXCERPT_BYTES,
};

/// Normalized artifact schema.
pub const ARTIFACT_SCHEMA: &str = "texo.code-index.v3";
const SCIP_DEFINITION: i32 = 0x1;
const SCIP_IMPORT: i32 = 0x2;
const SCIP_WRITE: i32 = 0x4;
const SCIP_READ: i32 = 0x8;
const SCIP_GENERATED: i32 = 0x10;
const SCIP_TEST: i32 = 0x20;
const SCIP_FORWARD_DEFINITION: i32 = 0x40;
const MAX_GAPS: usize = 256;
const MAX_LEXICAL_OCCURRENCES_PER_SOURCE: usize = 512;
static ARTIFACT_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Bounds for one code-index build/import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeIndexLimits {
    /// Maximum raw SCIP bytes accepted.
    pub max_scip_bytes: u64,
    /// Maximum documents consumed.
    pub max_documents: usize,
    /// Maximum normalized occurrences retained.
    pub max_occurrences: usize,
    /// Global wall budget for built-in analysis.
    pub analysis_budget: Duration,
}

impl Default for CodeIndexLimits {
    fn default() -> Self {
        Self {
            max_scip_bytes: 64 * 1024 * 1024,
            max_documents: 20_000,
            max_occurrences: 200_000,
            analysis_budget: Duration::from_secs(30),
        }
    }
}

/// A built artifact and the digest of its serialized bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCodeIndex {
    /// Normalized disposable artifact.
    pub artifact: CodeIndexArtifact,
    /// BLAKE3 digest of the exact persisted artifact bytes.
    pub artifact_digest_hex: String,
    /// Serialized artifact bytes.
    pub bytes: Vec<u8>,
}

/// Read a workspace-local SCIP file with a hard byte bound.
///
/// # Errors
/// Fails for paths outside the workspace, symlinks, non-regular files, and
/// files exceeding the declared bound.
pub fn read_scip(root: &Path, path: &Path, max_bytes: u64) -> Result<Vec<u8>, TexoError> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let canonical_root = std::fs::canonicalize(root)?;
    let canonical = std::fs::canonicalize(&candidate)?;
    if !canonical.starts_with(&canonical_root) {
        return Err(source_error(path, "SCIP path escapes the workspace"));
    }
    let metadata = std::fs::symlink_metadata(&candidate)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(source_error(
            path,
            "SCIP input must be a regular non-symlink file",
        ));
    }
    if metadata.len() > max_bytes {
        return Err(source_error(
            path,
            "SCIP input exceeds the configured byte limit",
        ));
    }
    let bytes = std::fs::read(&candidate)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != metadata.len() {
        return Err(source_error(path, "SCIP input changed while it was read"));
    }
    Ok(bytes)
}

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

/// Persist a normalized artifact with atomic content-addressed replacement.
///
/// # Errors
/// Returns an I/O error for staging, flush, rename, or directory sync failure.
pub fn persist(root: &Path, prepared: &PreparedCodeIndex) -> Result<PathBuf, TexoError> {
    let path = artifact_path(root, &prepared.artifact.index_id);
    atomic_write(&path, &prepared.bytes)?;
    Ok(path)
}

/// Load and authenticate one disposable normalized code index.
///
/// Missing artifacts return `Ok(None)` so callers can report degraded
/// coverage. Present but malformed or digest-mismatched artifacts fail closed.
///
/// # Errors
/// Returns a typed decode/source error when a present artifact is invalid.
pub fn load(
    root: &Path,
    index_id: &CodeIndexId,
    expected_digest_hex: &str,
) -> Result<Option<CodeIndexArtifact>, TexoError> {
    let path = artifact_path(root, index_id);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if blake3_bytes_hex(&bytes) != expected_digest_hex {
        return Err(source_error(&path, "code-index artifact digest mismatch"));
    }
    let artifact = batpak::encoding::from_bytes::<CodeIndexArtifact>(&bytes).map_err(|error| {
        TexoError::Decode {
            entity: index_id.to_string(),
            detail: error.to_string(),
        }
    })?;
    if artifact.schema != ARTIFACT_SCHEMA {
        return Ok(None);
    }
    if artifact.index_id != *index_id {
        return Err(source_error(&path, "code-index artifact identity mismatch"));
    }
    Ok(Some(artifact))
}

fn artifact_path(root: &Path, index_id: &CodeIndexId) -> PathBuf {
    root.join(".texo")
        .join("cache")
        .join("code-index")
        .join(format!("{}.bin", index_id.as_str()))
}

struct ArtifactBuilder {
    limits: CodeIndexLimits,
    occurrences: Vec<CodeOccurrence>,
    sources_examined: u64,
    truncated: bool,
    gaps: Vec<CoverageGap>,
    format: CodeIndexFormat,
    quality: AnalysisQuality,
}

impl ArtifactBuilder {
    fn new(limits: CodeIndexLimits, source_coverage: KnowledgeCoverage) -> Self {
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

    fn push(&mut self, occurrence: CodeOccurrence) -> bool {
        if self.occurrences.len() >= self.limits.max_occurrences {
            self.truncated = true;
            self.gap(None, CoverageGapKind::BudgetExceeded);
            return false;
        }
        self.occurrences.push(occurrence);
        true
    }

    fn gap(&mut self, path: Option<String>, kind: CoverageGapKind) {
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

fn import_scip(
    bytes: &[u8],
    sources: &BTreeMap<&str, &CapturedSource>,
    builder: &mut ArtifactBuilder,
    indexed_paths: &mut BTreeSet<String>,
) -> Result<String, TexoError> {
    let index = scip::types::Index::parse_from_bytes(bytes)
        .map_err(|error| source_error(Path::new("index.scip"), &error.to_string()))?;
    let analyzer = scip_analyzer_fingerprint(&index);
    for document in index.documents.iter().take(builder.limits.max_documents) {
        if !safe_relative_path(&document.relative_path) {
            builder.gap(
                Some(document.relative_path.clone()),
                CoverageGapKind::AnalysisIncomplete,
            );
            continue;
        }
        builder.sources_examined = builder.sources_examined.saturating_add(1);
        let Some(source) = sources.get(document.relative_path.as_str()).copied() else {
            builder.gap(
                Some(document.relative_path.clone()),
                CoverageGapKind::MissingObject,
            );
            continue;
        };
        if !position_is_utf8(document) {
            builder.gap(
                Some(document.relative_path.clone()),
                CoverageGapKind::UnsupportedEncoding,
            );
            continue;
        }
        if std::str::from_utf8(&source.bytes).is_err() {
            builder.gap(
                Some(document.relative_path.clone()),
                CoverageGapKind::UnsupportedEncoding,
            );
            continue;
        }
        indexed_paths.insert(document.relative_path.clone());
        import_scip_document(document, source, &analyzer, builder);
    }
    if index.documents.len() > builder.limits.max_documents {
        builder.truncated = true;
        builder.gap(None, CoverageGapKind::BudgetExceeded);
    }
    Ok(analyzer)
}

fn scip_analyzer_fingerprint(index: &scip::types::Index) -> String {
    let metadata = index.metadata.as_ref();
    let tool = metadata.and_then(|metadata| metadata.tool_info.as_ref());
    let name = tool.map_or("unknown", |tool| tool.name.as_str());
    let version = tool.map_or("unknown", |tool| tool.version.as_str());
    format!("scip:{name}:{version}:protocol-v0")
}

fn position_is_utf8(document: &scip::types::Document) -> bool {
    document.position_encoding.enum_value().ok()
        == Some(scip::types::PositionEncoding::UTF8CodeUnitOffsetFromLineStart)
}

fn import_scip_document(
    document: &scip::types::Document,
    source: &CapturedSource,
    analyzer: &str,
    builder: &mut ArtifactBuilder,
) {
    let lines = line_offsets(&source.bytes);
    let source_digest_hex = blake3_bytes_hex(&source.bytes);
    for occurrence in &document.occurrences {
        if occurrence.symbol.is_empty() {
            continue;
        }
        let Some(range) = scip_range(occurrence) else {
            builder.gap(
                Some(document.relative_path.clone()),
                CoverageGapKind::AnalysisIncomplete,
            );
            continue;
        };
        let Some((byte_range, line_range)) = resolve_range(&lines, &source.bytes, range) else {
            builder.gap(
                Some(document.relative_path.clone()),
                CoverageGapKind::AnalysisIncomplete,
            );
            continue;
        };
        let (Ok(start), Ok(end)) = (
            usize::try_from(byte_range.start),
            usize::try_from(byte_range.end),
        ) else {
            builder.gap(
                Some(document.relative_path.clone()),
                CoverageGapKind::AnalysisIncomplete,
            );
            continue;
        };
        let excerpt_bytes = &source.bytes[start..end];
        let Ok(excerpt) = std::str::from_utf8(excerpt_bytes) else {
            builder.gap(
                Some(document.relative_path.clone()),
                CoverageGapKind::UnsupportedEncoding,
            );
            continue;
        };
        if excerpt.len() > MAX_EVIDENCE_EXCERPT_BYTES {
            builder.gap(
                Some(document.relative_path.clone()),
                CoverageGapKind::SourceTooLarge,
            );
            continue;
        }
        let roles = scip_roles(occurrence.symbol_roles);
        let display_name = if excerpt.is_empty() {
            occurrence.symbol.clone()
        } else {
            excerpt.to_string()
        };
        let Some((context, context_byte_range, context_line_range)) =
            source_context(&source.bytes, start, end)
        else {
            builder.gap(
                Some(document.relative_path.clone()),
                CoverageGapKind::UnsupportedEncoding,
            );
            continue;
        };
        if !builder.push(CodeOccurrence {
            symbol: occurrence.symbol.clone(),
            display_name,
            roles,
            path: document.relative_path.clone(),
            byte_range,
            line_range,
            source_digest_hex: source_digest_hex.clone(),
            excerpt: excerpt.to_string(),
            context,
            context_byte_range,
            context_line_range,
            analyzer_fingerprint: analyzer.to_string(),
            analysis_quality: AnalysisQuality::Precise,
        }) {
            break;
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ZeroRange {
    start_line: usize,
    start_character: usize,
    end_line: usize,
    end_character: usize,
}

fn scip_range(occurrence: &scip::types::Occurrence) -> Option<ZeroRange> {
    use scip::types::occurrence::Typed_range;
    match occurrence.typed_range.as_ref() {
        Some(Typed_range::SingleLineRange(range)) => Some(ZeroRange {
            start_line: nonnegative(range.line)?,
            start_character: nonnegative(range.start_character)?,
            end_line: nonnegative(range.line)?,
            end_character: nonnegative(range.end_character)?,
        }),
        Some(Typed_range::MultiLineRange(range)) => Some(ZeroRange {
            start_line: nonnegative(range.start_line)?,
            start_character: nonnegative(range.start_character)?,
            end_line: nonnegative(range.end_line)?,
            end_character: nonnegative(range.end_character)?,
        }),
        None => match occurrence.range.as_slice() {
            [line, start, end] => Some(ZeroRange {
                start_line: nonnegative(*line)?,
                start_character: nonnegative(*start)?,
                end_line: nonnegative(*line)?,
                end_character: nonnegative(*end)?,
            }),
            [start_line, start, end_line, end] => Some(ZeroRange {
                start_line: nonnegative(*start_line)?,
                start_character: nonnegative(*start)?,
                end_line: nonnegative(*end_line)?,
                end_character: nonnegative(*end)?,
            }),
            _ => None,
        },
        Some(_) => None,
    }
}

fn nonnegative(value: i32) -> Option<usize> {
    usize::try_from(value).ok()
}

fn line_offsets(bytes: &[u8]) -> Vec<usize> {
    let mut offsets = vec![0];
    for (offset, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            offsets.push(offset.saturating_add(1));
        }
    }
    offsets
}

fn resolve_range(
    lines: &[usize],
    bytes: &[u8],
    range: ZeroRange,
) -> Option<(ByteRange, LineRange)> {
    let start = lines
        .get(range.start_line)?
        .checked_add(range.start_character)?;
    let end = lines
        .get(range.end_line)?
        .checked_add(range.end_character)?;
    if start > end || end > bytes.len() {
        return None;
    }
    let byte_range = ByteRange::new(u64::try_from(start).ok()?, u64::try_from(end).ok()?).ok()?;
    let line_range = LineRange::new(
        u32::try_from(range.start_line.checked_add(1)?).ok()?,
        u32::try_from(range.end_line.checked_add(1)?).ok()?,
    )
    .ok()?;
    Some((byte_range, line_range))
}

fn scip_roles(bits: i32) -> Vec<CodeOccurrenceRole> {
    let mut roles = Vec::new();
    if bits & SCIP_DEFINITION != 0 {
        roles.push(CodeOccurrenceRole::Definition);
    } else {
        roles.push(CodeOccurrenceRole::Reference);
    }
    for (mask, role) in [
        (SCIP_IMPORT, CodeOccurrenceRole::Import),
        (SCIP_WRITE, CodeOccurrenceRole::Write),
        (SCIP_READ, CodeOccurrenceRole::Read),
        (SCIP_GENERATED, CodeOccurrenceRole::Generated),
        (SCIP_TEST, CodeOccurrenceRole::Test),
        (
            SCIP_FORWARD_DEFINITION,
            CodeOccurrenceRole::ForwardDefinition,
        ),
    ] {
        if bits & mask != 0 {
            roles.push(role);
        }
    }
    roles
}

fn analyze_fallbacks(
    sources: &[CapturedSource],
    indexed_paths: &BTreeSet<String>,
    builder: &mut ArtifactBuilder,
    budget: Duration,
) -> Result<String, TexoError> {
    let deadline = Instant::now() + budget;
    let mut used_syntax = false;
    let mut used_lexical = false;
    for source in sources {
        if indexed_paths.contains(&source.path) {
            continue;
        }
        if !code_index_path_is_in_scope(&source.path) {
            continue;
        }
        if Instant::now() >= deadline {
            builder.truncated = true;
            builder.gap(None, CoverageGapKind::BudgetExceeded);
            break;
        }
        builder.sources_examined = builder.sources_examined.saturating_add(1);
        if std::str::from_utf8(&source.bytes).is_err() {
            builder.gap(
                Some(source.path.clone()),
                CoverageGapKind::UnsupportedEncoding,
            );
            continue;
        }
        #[cfg(feature = "code-rust")]
        if Path::new(&source.path)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
        {
            if analyze_rust(source, builder, deadline)? {
                used_syntax = true;
                continue;
            }
            builder.gap(
                Some(source.path.clone()),
                CoverageGapKind::AnalysisIncomplete,
            );
        }
        analyze_lexical(source, builder);
        used_lexical = true;
    }
    if used_syntax && builder.format == CodeIndexFormat::Lexical {
        builder.quality = AnalysisQuality::Syntactic;
        builder.format = CodeIndexFormat::Syntax;
    }
    Ok(match (used_syntax, used_lexical) {
        (true, true) => format!("{}+texo-lexical:v2", rust_analyzer_fingerprint()),
        (true, false) => rust_analyzer_fingerprint(),
        (false, true) => "texo-lexical:v2".to_string(),
        (false, false) => String::new(),
    })
}

#[cfg(feature = "code-rust")]
fn analyze_rust(
    source: &CapturedSource,
    builder: &mut ArtifactBuilder,
    deadline: Instant,
) -> Result<bool, TexoError> {
    use tree_sitter::Parser;
    let language = tree_sitter_rust::LANGUAGE.into();
    let mut parser = Parser::new();
    parser.set_language(&language).map_err(|error| {
        source_error(Path::new(&source.path), &format!("Rust grammar: {error}"))
    })?;
    let len = source.bytes.len();
    let mut read = |offset: usize, _| {
        if offset < len {
            &source.bytes[offset..]
        } else {
            &[]
        }
    };
    let mut progress = |_: &tree_sitter::ParseState| {
        if Instant::now() >= deadline {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = tree_sitter::ParseOptions::new().progress_callback(&mut progress);
    let Some(tree) = parser.parse_with_options(&mut read, None, Some(options)) else {
        return Ok(false);
    };
    if tree.root_node().has_error() {
        builder.gap(
            Some(source.path.clone()),
            CoverageGapKind::AnalysisIncomplete,
        );
    }
    collect_rust_tags(source, builder, &language, &tree)?;
    Ok(true)
}

#[cfg(feature = "code-rust")]
fn collect_rust_tags(
    source: &CapturedSource,
    builder: &mut ArtifactBuilder,
    language: &tree_sitter::Language,
    tree: &tree_sitter::Tree,
) -> Result<(), TexoError> {
    use tree_sitter::{Query, QueryCursor, StreamingIterator};
    let query = Query::new(language, tree_sitter_rust::TAGS_QUERY).map_err(|error| {
        source_error(
            Path::new(&source.path),
            &format!("Rust tags query: {error}"),
        )
    })?;
    let mut cursor = QueryCursor::new();
    cursor.set_match_limit(4096);
    let source_digest_hex = blake3_bytes_hex(&source.bytes);
    let analyzer = rust_analyzer_fingerprint();
    let capture_names = query.capture_names();
    let mut matches = cursor.matches(&query, tree.root_node(), source.bytes.as_slice());
    while let Some(item) = matches.next() {
        let role = item.captures.iter().find_map(|capture| {
            let index = usize::try_from(capture.index).ok()?;
            let name = capture_names.get(index)?;
            if name.starts_with("definition.") {
                Some((CodeOccurrenceRole::Definition, *name))
            } else if name.starts_with("reference.") {
                Some((CodeOccurrenceRole::Reference, *name))
            } else {
                None
            }
        });
        let name_capture = item.captures.iter().find(|capture| {
            usize::try_from(capture.index)
                .ok()
                .and_then(|index| capture_names.get(index))
                .copied()
                == Some("name")
        });
        let (Some((role, kind)), Some(name_capture)) = (role, name_capture) else {
            continue;
        };
        let node = name_capture.node;
        let Ok(display_name) = node.utf8_text(&source.bytes) else {
            builder.gap(
                Some(source.path.clone()),
                CoverageGapKind::UnsupportedEncoding,
            );
            continue;
        };
        let symbol = format!(
            "syntax rust {}#{}:{}@{}",
            source.path,
            kind,
            display_name,
            node.start_byte()
        );
        let Some((context, context_byte_range, context_line_range)) =
            source_context(&source.bytes, node.start_byte(), node.end_byte())
        else {
            builder.gap(
                Some(source.path.clone()),
                CoverageGapKind::UnsupportedEncoding,
            );
            continue;
        };
        let occurrence = CodeOccurrence {
            symbol,
            display_name: display_name.to_string(),
            roles: vec![role],
            path: source.path.clone(),
            byte_range: ByteRange::new(
                u64::try_from(node.start_byte()).unwrap_or(u64::MAX),
                u64::try_from(node.end_byte()).unwrap_or(u64::MAX),
            )
            .map_err(|error| source_error(Path::new(&source.path), &error.to_string()))?,
            line_range: LineRange::new(
                u32::try_from(node.start_position().row.saturating_add(1)).unwrap_or(u32::MAX),
                u32::try_from(node.end_position().row.saturating_add(1)).unwrap_or(u32::MAX),
            )
            .map_err(|error| source_error(Path::new(&source.path), &error.to_string()))?,
            source_digest_hex: source_digest_hex.clone(),
            excerpt: display_name.to_string(),
            context,
            context_byte_range,
            context_line_range,
            analyzer_fingerprint: analyzer.clone(),
            analysis_quality: AnalysisQuality::Syntactic,
        };
        if !builder.push(occurrence) {
            break;
        }
    }
    if cursor.did_exceed_match_limit() {
        builder.truncated = true;
        builder.gap(Some(source.path.clone()), CoverageGapKind::BudgetExceeded);
    }
    Ok(())
}

#[cfg(feature = "code-rust")]
fn rust_analyzer_fingerprint() -> String {
    let query_digest = blake3_bytes_hex(tree_sitter_rust::TAGS_QUERY.as_bytes());
    format!("tree-sitter:0.26.11:rust:0.24.2:tags-{query_digest}")
}

#[cfg(not(feature = "code-rust"))]
fn rust_analyzer_fingerprint() -> String {
    "tree-sitter-rust:disabled".to_string()
}

fn analyze_lexical(source: &CapturedSource, builder: &mut ArtifactBuilder) {
    let source_digest_hex = blake3_bytes_hex(&source.bytes);
    let mut names = BTreeSet::new();
    let mut offset = 0;
    while offset < source.bytes.len() {
        if !identifier_start(source.bytes[offset]) {
            offset += 1;
            continue;
        }
        let start = offset;
        offset += 1;
        while offset < source.bytes.len() && identifier_continue(source.bytes[offset]) {
            offset += 1;
        }
        let Ok(name) = std::str::from_utf8(&source.bytes[start..offset]) else {
            continue;
        };
        if name.len() < 3 || !names.insert(name.to_ascii_lowercase()) {
            continue;
        }
        if names.len() > MAX_LEXICAL_OCCURRENCES_PER_SOURCE {
            builder.truncated = true;
            builder.gap(Some(source.path.clone()), CoverageGapKind::BudgetExceeded);
            return;
        }
        let (start_line, end_line) = byte_line_range(&source.bytes, start, offset);
        let Some((context, context_byte_range, context_line_range)) =
            source_context(&source.bytes, start, offset)
        else {
            builder.gap(
                Some(source.path.clone()),
                CoverageGapKind::UnsupportedEncoding,
            );
            continue;
        };
        let occurrence = CodeOccurrence {
            symbol: format!("lexical {}#{}@{start}", source.path, name),
            display_name: name.to_string(),
            roles: vec![CodeOccurrenceRole::Reference],
            path: source.path.clone(),
            byte_range: ByteRange {
                start: u64::try_from(start).unwrap_or(u64::MAX),
                end: u64::try_from(offset).unwrap_or(u64::MAX),
            },
            line_range: LineRange {
                start: start_line,
                end: end_line,
            },
            source_digest_hex: source_digest_hex.clone(),
            excerpt: name.to_string(),
            context,
            context_byte_range,
            context_line_range,
            analyzer_fingerprint: "texo-lexical:v2".to_string(),
            analysis_quality: AnalysisQuality::Lexical,
        };
        if !builder.push(occurrence) {
            return;
        }
    }
}

fn code_index_path_is_in_scope(path: &str) -> bool {
    let path = Path::new(path);
    if path.components().any(|component| {
        component.as_os_str().to_str().is_some_and(|name| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                "dist" | "build" | "node_modules" | ".next" | "coverage"
            )
        })
    }) || path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                "pnpm-lock.yaml" | "pnpm-lock.yml"
            )
        })
    {
        return false;
    }
    // Extensionless build/config files are in the Git capture scope, so they must
    // be indexable here too or a captured source would silently produce neither an
    // occurrence nor a gap.
    if path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(crate::git_source::is_wellknown_source_basename)
    {
        return true;
    }
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
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
                    | "c"
                    | "h"
                    | "cc"
                    | "cpp"
                    | "hpp"
                    | "sh"
                    | "toml"
                    | "yaml"
                    | "yml"
                    | "json"
            )
        })
}

fn identifier_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn identifier_continue(byte: u8) -> bool {
    identifier_start(byte) || byte.is_ascii_digit()
}

fn source_context(
    bytes: &[u8],
    occurrence_start: usize,
    occurrence_end: usize,
) -> Option<(String, ByteRange, LineRange)> {
    if occurrence_start > occurrence_end || occurrence_end > bytes.len() {
        return None;
    }
    let line_start = bytes[..occurrence_start]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position.saturating_add(1));
    let line_end = bytes[occurrence_end..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |position| {
            occurrence_end.saturating_add(position)
        });
    let occurrence_len = occurrence_end.saturating_sub(occurrence_start);
    if occurrence_len > MAX_EVIDENCE_EXCERPT_BYTES {
        return None;
    }
    let available = MAX_EVIDENCE_EXCERPT_BYTES.saturating_sub(occurrence_len);
    let before = available / 2;
    let mut start = occurrence_start.saturating_sub(before).max(line_start);
    let mut end = occurrence_end
        .saturating_add(available.saturating_sub(occurrence_start.saturating_sub(start)))
        .min(line_end);
    if end.saturating_sub(start) < MAX_EVIDENCE_EXCERPT_BYTES {
        start = end
            .saturating_sub(MAX_EVIDENCE_EXCERPT_BYTES)
            .max(line_start);
    }
    while start < occurrence_start && bytes.get(start).is_some_and(|byte| byte & 0xc0 == 0x80) {
        start = start.saturating_add(1);
    }
    while end > occurrence_end && bytes.get(end).is_some_and(|byte| byte & 0xc0 == 0x80) {
        end = end.saturating_sub(1);
    }
    let context = std::str::from_utf8(&bytes[start..end]).ok()?.to_string();
    let (start_line, end_line) = byte_line_range(bytes, start, end);
    Some((
        context,
        ByteRange {
            start: u64::try_from(start).unwrap_or(u64::MAX),
            end: u64::try_from(end).unwrap_or(u64::MAX),
        },
        LineRange {
            start: start_line,
            end: end_line,
        },
    ))
}

fn byte_line_range(bytes: &[u8], start: usize, end: usize) -> (u32, u32) {
    let start_line =
        1_u32.saturating_add(u32::try_from(count_newlines(&bytes[..start])).unwrap_or(u32::MAX));
    let end_line = start_line
        .saturating_add(u32::try_from(count_newlines(&bytes[start..end])).unwrap_or(u32::MAX));
    (start_line, end_line)
}

fn count_newlines(bytes: &[u8]) -> usize {
    let mut count = 0_usize;
    for byte in bytes {
        if *byte == b'\n' {
            count = count.saturating_add(1);
        }
    }
    count
}

fn code_occurrence_order(left: &CodeOccurrence, right: &CodeOccurrence) -> std::cmp::Ordering {
    left.symbol
        .cmp(&right.symbol)
        .then_with(|| left.path.cmp(&right.path))
        .then_with(|| left.byte_range.start.cmp(&right.byte_range.start))
        .then_with(|| left.byte_range.end.cmp(&right.byte_range.end))
        .then_with(|| left.roles.cmp(&right.roles))
}

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !Path::new(path).is_absolute()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("artifact path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    for _attempt in 0..100 {
        let counter = ARTIFACT_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(
            ".texo-code-index-{}-{counter}.tmp",
            std::process::id()
        ));
        let mut file = match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, path)?;
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _removed = std::fs::remove_file(&tmp);
        }
        return result;
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a private code-index staging file",
    ))
}

fn source_error(path: &Path, detail: &str) -> TexoError {
    TexoError::Source {
        path: path.display().to_string(),
        detail: detail.to_string(),
    }
}
