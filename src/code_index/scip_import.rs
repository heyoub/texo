use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use protobuf::Message;

use crate::error::TexoError;
use crate::events::ids::blake3_bytes_hex;
use crate::git_source::CapturedSource;
use crate::knowledge::{
    AnalysisQuality, ByteRange, CodeOccurrence, CodeOccurrenceRole, CoverageGapKind, LineRange,
    MAX_EVIDENCE_EXCERPT_BYTES,
};

use super::artifact::ArtifactBuilder;
use super::util::{safe_relative_path, source_context, source_error};

const SCIP_DEFINITION: i32 = 0x1;
const SCIP_IMPORT: i32 = 0x2;
const SCIP_WRITE: i32 = 0x4;
const SCIP_READ: i32 = 0x8;
const SCIP_GENERATED: i32 = 0x10;
const SCIP_TEST: i32 = 0x20;
const SCIP_FORWARD_DEFINITION: i32 = 0x40;

pub(super) fn import_scip(
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
        let before = builder.occurrences.len();
        import_scip_document(document, source, &analyzer, builder);
        // Only claim the path as SCIP-indexed when the document actually produced
        // occurrences. A matching-but-empty document must fall through to the
        // built-in fallback rather than suppress every symbol for that file.
        if builder.occurrences.len() > before {
            indexed_paths.insert(document.relative_path.clone());
        }
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
