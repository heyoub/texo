use std::collections::BTreeSet;
#[cfg(feature = "code-rust")]
use std::ops::ControlFlow;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::TexoError;
use crate::events::ids::blake3_bytes_hex;
use crate::git_source::CapturedSource;
use crate::knowledge::{
    AnalysisQuality, ByteRange, CodeIndexFormat, CodeOccurrence, CodeOccurrenceRole,
    CoverageGapKind, LineRange,
};

use super::artifact::ArtifactBuilder;
#[cfg(feature = "code-rust")]
use super::util::source_error;
use super::util::{byte_line_range, source_context};

const MAX_LEXICAL_OCCURRENCES_PER_SOURCE: usize = 512;

pub(super) fn analyze_fallbacks(
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
