//! Markdown source parsing.

use std::path::{Path, PathBuf};

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::events::ids::{blake3_bytes_hex, source_id_from_hash, IdParseError};

/// Maximum source-line size admitted to claim extraction. Longer physical
/// lines remain covered by the source hash but are not materialized as claims.
pub const MAX_EXTRACTABLE_LINE_BYTES: usize = 64 * 1024;

/// One logical line in a markdown document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownLine {
    /// 1-based line number.
    pub number: u32,
    /// Raw line text.
    pub text: String,
    /// Byte offset of the line's first byte into the source body, so
    /// `source[char_start..char_start + text.len()] == text`. Line terminators
    /// (`\n` / `\r\n`) are not part of `text` and sit past that range.
    pub char_start: usize,
}

/// Parsed markdown source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownDocument {
    /// Relative or absolute path string.
    pub path: String,
    /// Body bytes hash hex.
    pub body_hash_hex: String,
    /// Deterministic source id.
    pub source_id: String,
    /// Extractable lines.
    pub lines: Vec<MarkdownLine>,
}

impl MarkdownDocument {
    /// Parse markdown from disk.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError::Io`] when the file cannot be read; otherwise the
    /// [`Self::from_bytes`] errors on its content.
    pub fn from_path(path: &Path, root: &Path) -> Result<Self, SourceError> {
        let bytes = std::fs::read(path).map_err(SourceError::Io)?;
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        Self::from_bytes(&rel, &bytes)
    }

    /// Parse markdown bytes.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError::Utf8`] when `bytes` are not valid UTF-8;
    /// [`SourceError::Id`] when a source id cannot be derived from the body hash.
    pub fn from_bytes(path: &str, bytes: &[u8]) -> Result<Self, SourceError> {
        let body_hash_hex = blake3_bytes_hex(bytes);
        let source_id = source_id_from_hash(&body_hash_hex)?.to_string();
        let text = String::from_utf8(bytes.to_vec())?;
        let lines = parse_lines(&text);
        Ok(Self {
            path: path.to_string(),
            body_hash_hex,
            source_id,
            lines,
        })
    }
}

fn parse_lines(text: &str) -> Vec<MarkdownLine> {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let mut skipping_frontmatter = false;
    let mut frontmatter_done = false;

    // Iterate with `split_inclusive('\n')` (terminators kept) so each line's
    // byte offset is the running sum of the previous segments. Stripping the
    // trailing `\n` / `\r\n` reproduces exactly what `text.lines()` yields, so
    // the retained-line filtering below is unchanged.
    let mut offset = 0usize;
    for (idx, segment) in text.split_inclusive('\n').enumerate() {
        let char_start = offset;
        offset += segment.len();

        // Match `str::lines()` exactly: strip a trailing `\n`, and a `\r` only
        // when it immediately preceded that `\n` (a lone `\r` at EOF stays).
        let raw = match segment.strip_suffix('\n') {
            Some(no_newline) => no_newline.strip_suffix('\r').unwrap_or(no_newline),
            None => segment,
        };

        if raw.len() > MAX_EXTRACTABLE_LINE_BYTES {
            continue;
        }

        let number = u32::try_from(idx + 1).unwrap_or(u32::MAX);
        if !frontmatter_done && idx == 0 && raw.trim() == "---" {
            skipping_frontmatter = true;
            continue;
        }
        if skipping_frontmatter {
            if raw.trim() == "---" {
                skipping_frontmatter = false;
                frontmatter_done = true;
            }
            continue;
        }

        if raw.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }

        lines.push(MarkdownLine {
            number,
            text: raw.to_string(),
            char_start,
        });
    }
    lines
}

/// A prose block that may carry a durable claim.
///
/// Candidates are emitted by [`segment_candidates`] from the `CommonMark` AST.
/// Structural and non-durable nodes (headings, code, frontmatter, HTML, tables,
/// blockquotes) are excluded; only paragraphs and list items survive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateSpan {
    /// The verbatim source text of the block.
    pub text: String,
    /// 1-based raw file line of the first byte.
    pub line_start: u32,
    /// 1-based raw file line of the last byte.
    pub line_end: u32,
    /// Byte offset of the block start into the source.
    pub char_start: usize,
    /// Byte offset of the block end (exclusive) into the source.
    pub char_end: usize,
    /// Trail of enclosing ATX/Setext headings, outermost first.
    pub heading_path: Vec<String>,
}

/// One frame of the heading stack: its nesting level and rendered text.
struct HeadingFrame {
    level: HeadingLevel,
    text: String,
}

/// Segment a markdown source into prose candidate spans.
///
/// Emits one [`CandidateSpan`] per paragraph and per list item that can carry a
/// durable claim. Headings, fenced and indented code, frontmatter, HTML blocks,
/// tables, and blockquotes (and all their contents) are excluded. Empty or
/// degenerate input yields an empty vector.
#[must_use]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "pulldown-cmark Event is a foreign enum that grows variants; only the matched events matter here"
)]
pub fn segment_candidates(source: &str) -> Vec<CandidateSpan> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    options.insert(Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS);

    let bounded_source = mask_oversized_lines(source);
    let source = bounded_source.as_ref();
    let mut spans = Vec::new();
    let mut headings: Vec<HeadingFrame> = Vec::new();

    // While inside an excluded container (heading, code, table, blockquote,
    // HTML, frontmatter) we suppress candidate emission and accumulate heading
    // text. `suppress_depth` counts nested excluded containers; emission is only
    // allowed when it is zero.
    let mut suppress_depth: usize = 0;
    // Set when we are inside a heading and should capture its text.
    let mut capturing_heading: Option<(HeadingLevel, String)> = None;
    // The outermost prose block currently open (paragraph or list item) and its
    // byte range. Nested prose (e.g. a paragraph inside a list item) does not
    // open a new candidate — the outer block already covers it.
    let mut open_block: Option<(usize, usize)> = None;

    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                suppress_depth += 1;
                capturing_heading = Some((level, String::new()));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, text)) = capturing_heading.take() {
                    while headings.last().is_some_and(|frame| frame.level >= level) {
                        headings.pop();
                    }
                    headings.push(HeadingFrame {
                        level,
                        text: text.trim().to_string(),
                    });
                }
                suppress_depth = suppress_depth.saturating_sub(1);
            }
            Event::Start(
                Tag::CodeBlock(_)
                | Tag::Table(_)
                | Tag::BlockQuote(_)
                | Tag::HtmlBlock
                | Tag::MetadataBlock(_),
            ) => {
                suppress_depth += 1;
            }
            Event::End(
                TagEnd::CodeBlock
                | TagEnd::Table
                | TagEnd::BlockQuote(_)
                | TagEnd::HtmlBlock
                | TagEnd::MetadataBlock(_),
            ) => {
                suppress_depth = suppress_depth.saturating_sub(1);
            }
            Event::Start(Tag::Paragraph | Tag::Item) => {
                if suppress_depth == 0 && open_block.is_none() {
                    open_block = Some((range.start, range.end));
                }
            }
            Event::End(TagEnd::Paragraph | TagEnd::Item) => {
                if suppress_depth == 0 {
                    if let Some((start, end)) = open_block.take() {
                        push_span(&mut spans, source, start, end, &headings);
                    }
                }
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, buf)) = capturing_heading.as_mut() {
                    buf.push_str(&text);
                }
            }
            _ => {}
        }
    }

    spans
}

fn mask_oversized_lines(source: &str) -> std::borrow::Cow<'_, str> {
    if !source
        .split_inclusive('\n')
        .any(|line| line.trim_end_matches(['\r', '\n']).len() > MAX_EXTRACTABLE_LINE_BYTES)
    {
        return std::borrow::Cow::Borrowed(source);
    }
    let mut bytes = source.as_bytes().to_vec();
    let mut start = 0;
    while start < bytes.len() {
        let end = bytes[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |relative| start + relative);
        let content_end = end - usize::from(end > start && bytes[end - 1] == b'\r');
        if content_end.saturating_sub(start) > MAX_EXTRACTABLE_LINE_BYTES {
            bytes[start..content_end].fill(b' ');
        }
        start = end.saturating_add(1);
    }
    // Replacing bytes only with ASCII spaces preserves UTF-8 validity and all
    // subsequent source offsets.
    let masked = String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
    std::borrow::Cow::Owned(masked)
}

/// Build and record a candidate span from a byte range, trimming trailing
/// whitespace/newlines so offsets slice back exactly to the emitted text.
fn push_span(
    spans: &mut Vec<CandidateSpan>,
    source: &str,
    start: usize,
    end: usize,
    headings: &[HeadingFrame],
) {
    let raw = &source[start..end];
    let trimmed = raw.trim_end_matches(['\n', '\r', ' ', '\t']);
    if trimmed.is_empty() {
        return;
    }
    let char_end = start + trimmed.len();
    let line_start = line_of_offset(source, start);
    let line_end = line_of_offset(source, char_end.saturating_sub(1));
    spans.push(CandidateSpan {
        text: trimmed.to_string(),
        line_start,
        line_end,
        char_start: start,
        char_end,
        heading_path: headings.iter().map(|f| f.text.clone()).collect(),
    });
}

/// Compute the 1-based file line of a byte offset by counting newlines before it.
///
/// Counts over the **byte** slice rather than a `str` slice: `offset` may fall
/// inside a multi-byte character (e.g. one byte before the end of an em-dash), and
/// `&source[..offset]` would panic on a non-char-boundary index. Byte indexing has
/// no such requirement, and newline (`\n`) is single-byte and never part of a
/// multi-byte sequence, so the count is identical and panic-free.
fn line_of_offset(source: &str, offset: usize) -> u32 {
    let upto = offset.min(source.len());
    // `bytes().take(upto)` avoids slicing the `str` at `upto` (which may be a
    // non-char-boundary and panic) while still counting only bytes before it.
    let count = source.bytes().take(upto).filter(|&b| b == b'\n').count() + 1;
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Source parsing errors.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Invalid UTF-8.
    #[error("utf8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    /// Failed to derive a source id from the body hash.
    #[error("id: {0}")]
    Id(#[from] IdParseError),
    /// Directory traversal failed.
    #[error("walk: {0}")]
    Walk(#[from] walkdir::Error),
}

/// One unreadable descendant discovered under an otherwise valid root.
#[derive(Debug)]
pub struct DiscoveryFailure {
    /// Best available path.
    pub path: PathBuf,
    /// Traversal error.
    pub error: walkdir::Error,
}

/// Deterministically sorted markdown discovery plus descendant failures.
#[derive(Debug, Default)]
pub struct MarkdownDiscovery {
    /// Markdown files beneath the requested root.
    pub files: Vec<PathBuf>,
    /// Unreadable descendants. Root failures are returned as [`SourceError`].
    pub failures: Vec<DiscoveryFailure>,
}

/// Collect markdown files under a directory.
///
/// # Errors
///
/// Missing paths and unreadable roots are hard errors. Unreadable descendants
/// are retained as typed failure rows so tolerant ingest can settle them.
pub fn collect_markdown_files(input: &Path) -> Result<MarkdownDiscovery, SourceError> {
    if !input.exists() {
        return Err(SourceError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("source root does not exist: {}", input.display()),
        )));
    }
    let mut files = Vec::new();
    if input.is_file() {
        if is_markdown(input) {
            files.push(input.to_path_buf());
        }
        return Ok(MarkdownDiscovery {
            files,
            failures: Vec::new(),
        });
    }
    let mut failures = Vec::new();
    for result in walkdir::WalkDir::new(input) {
        match result {
            Ok(entry) => {
                let path = entry.path();
                if path.is_file() && is_markdown(path) {
                    files.push(path.to_path_buf());
                }
            }
            Err(error) if error.depth() == 0 => return Err(SourceError::Walk(error)),
            Err(error) => failures.push(DiscoveryFailure {
                path: error
                    .path()
                    .map_or_else(|| input.to_path_buf(), Path::to_path_buf),
                error,
            }),
        }
    }
    files.sort();
    failures.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(MarkdownDiscovery { files, failures })
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn skips_code_fence() {
        // Lines (1-based):
        //   1 `# Title`      kept
        //   2 ``            kept (blank)
        //   3 ```` ``` ````  fence open  -> dropped
        //   4 `code`         inside fence -> dropped
        //   5 ```` ``` ````  fence close -> dropped
        //   6 ``            kept (blank)
        //   7 `Deploys...`   kept
        let doc = MarkdownDocument::from_bytes(
            "test.md",
            b"# Title\n\n```\ncode\n```\n\nDeploys happen on Friday.\n",
        )
        .expect("parse");

        // Exactly the four non-fenced lines survive, with their ORIGINAL 1-based
        // numbers preserved (so claim provenance points at the real file line).
        let surviving: Vec<(u32, &str)> = doc
            .lines
            .iter()
            .map(|l| (l.number, l.text.as_str()))
            .collect();
        // Exactly the four non-fenced lines survive with their original numbers.
        assert_eq!(
            surviving,
            vec![
                (1, "# Title"),
                (2, ""),
                (6, ""),
                (7, "Deploys happen on Friday.")
            ]
        );

        // The fence delimiters themselves and the fenced body must all be gone.
        let kept_numbers: Vec<u32> = doc.lines.iter().map(|l| l.number).collect();
        for fenced in [3u32, 4, 5] {
            assert!(!kept_numbers.contains(&fenced));
        }
        assert!(doc
            .lines
            .iter()
            .all(|l| !l.text.contains("code") && !l.text.trim_start().starts_with("```")));
    }

    #[test]
    fn skips_yaml_frontmatter_block_and_preserves_line_numbers() {
        // Frontmatter occupies lines 1-3; body starts at line 4. The parser must
        // drop the entire `---`-delimited block (including its delimiters) yet
        // keep the surviving body lines at their true file line numbers.
        let doc = MarkdownDocument::from_bytes(
            "fm.md",
            b"---\ntitle: X\n---\n# Heading\n\nDeploys happen on Friday.\n",
        )
        .expect("parse");

        let surviving: Vec<(u32, &str)> = doc
            .lines
            .iter()
            .map(|l| (l.number, l.text.as_str()))
            .collect();
        // Frontmatter (lines 1-3) is excluded; body keeps its real line numbers.
        assert_eq!(
            surviving,
            vec![(4, "# Heading"), (5, ""), (6, "Deploys happen on Friday.")]
        );
        assert!(doc
            .lines
            .iter()
            .all(|l| l.text != "title: X" && l.text != "---"));
    }

    #[test]
    fn line_char_start_slices_source_back_to_line_text() {
        // Every retained line's byte offset must slice the ORIGINAL source back
        // to exactly that line's text, across frontmatter (dropped), fences
        // (dropped), CRLF terminators, and multi-byte characters.
        let source =
            "---\ntitle: X\n---\n# Heading\r\n\n```\ncode\n```\nDeploys — happen on Friday.\n";
        let doc = MarkdownDocument::from_bytes("offsets.md", source.as_bytes()).expect("parse");
        assert!(!doc.lines.is_empty());
        for line in &doc.lines {
            assert_eq!(
                &source[line.char_start..line.char_start + line.text.len()],
                line.text,
                "line {} offset must slice back to its text",
                line.number
            );
        }
        // Spot-check the prose line: it follows the fence and keeps its real
        // byte position in the raw file.
        let prose = doc
            .lines
            .iter()
            .find(|l| l.text.contains("Friday"))
            .expect("prose line retained");
        let expected = source.find("Deploys").expect("prose exists in source");
        assert_eq!(prose.char_start, expected);
    }

    #[test]
    fn from_bytes_rejects_invalid_utf8_with_source_error() {
        // A lone 0xFF byte is never valid UTF-8. The hash and source id are
        // derived from raw bytes (so they succeed), but decoding to text must
        // fail with SourceError::Utf8 rather than panicking or lossily decoding.
        let err = MarkdownDocument::from_bytes("bad.md", &[0xFF, 0xFE, 0x00])
            .expect_err("invalid utf-8 must error");
        assert_matches!(&err, SourceError::Utf8(_));
        // The Display impl must label the variant for diagnosability.
        assert!(err.to_string().starts_with("utf8: "));
    }

    #[test]
    fn from_path_missing_file_is_io_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does_not_exist.md");
        let err =
            MarkdownDocument::from_path(&missing, dir.path()).expect_err("missing file must error");
        assert_matches!(err, SourceError::Io(_));
    }

    /// Load a real Helios doc relative to this crate's manifest directory.
    fn helios_doc(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/helios/docs")
            .join(name);
        let context = format!("read helios doc {}", path.display());
        std::fs::read_to_string(&path).expect(&context)
    }

    #[test]
    fn segment_excludes_heading_noise() {
        // The ATX heading "## Decision" is structural noise: it must never be
        // emitted as a stand-alone candidate span.
        let source = helios_doc("07_storage_adr.md");
        let spans = segment_candidates(&source);
        assert!(
            !spans.iter().any(|s| s.text.trim() == "## Decision"),
            "heading text must not be a candidate span"
        );
        // No span should be a bare ATX heading line at all.
        assert!(
            !spans.iter().any(|s| s.text.trim_start().starts_with("## ")),
            "no candidate should be a bare ATX heading"
        );
    }

    #[test]
    fn segment_excludes_fenced_code_and_headings() {
        // The onboarding wiki has a ```yaml``` fence containing `approver: alice`
        // and several ATX headings. None of that content is durable prose.
        let source = helios_doc("01_onboarding_wiki.md");
        let spans = segment_candidates(&source);
        assert!(
            !spans.iter().any(|s| s.text.contains("approver: alice")),
            "fenced code content must be excluded"
        );
        assert!(
            !spans.iter().any(|s| {
                let t = s.text.trim_start();
                t.starts_with("## ") || t.starts_with("# ")
            }),
            "bare ATX headings must be excluded"
        );
    }

    #[test]
    fn segment_includes_prose_with_heading_path() {
        // Prose under "## Shipping your first change" must be a candidate, and
        // carry that heading in its heading_path.
        let source = helios_doc("01_onboarding_wiki.md");
        let spans = segment_candidates(&source);
        let hit = spans
            .iter()
            .find(|s| s.text.contains("Deploys happen on Friday"))
            .expect("prose span must be emitted");
        assert!(
            hit.heading_path
                .iter()
                .any(|h| h == "Shipping your first change"),
            "heading_path must include the enclosing ATX heading, got {:?}",
            hit.heading_path
        );
    }

    #[test]
    fn segment_includes_decision_prose_under_heading() {
        // The ADR's decision prose ("uses BatPak") is durable and sits under the
        // "Decision" heading.
        let source = helios_doc("07_storage_adr.md");
        let spans = segment_candidates(&source);
        let hit = spans
            .iter()
            .find(|s| s.text.contains("uses BatPak"))
            .expect("decision prose span must be emitted");
        assert!(
            hit.heading_path.iter().any(|h| h == "Decision"),
            "heading_path must include Decision, got {:?}",
            hit.heading_path
        );
    }

    #[test]
    fn segment_offsets_slice_back_and_lines_match() {
        let source = helios_doc("01_onboarding_wiki.md");
        let spans = segment_candidates(&source);
        let hit = spans
            .iter()
            .find(|s| s.text.contains("Deploys happen on Friday"))
            .expect("prose span must be emitted");

        // char_start..char_end must slice the source back to the span text.
        assert_eq!(
            &source[hit.char_start..hit.char_end],
            hit.text,
            "byte offsets must slice back to the span text"
        );

        // line_start must be the real 1-based file line of the first byte.
        let computed_line = source[..hit.char_start]
            .bytes()
            .filter(|&b| b == b'\n')
            .count()
            + 1;
        assert_eq!(
            usize::try_from(hit.line_start).expect("line fits usize"),
            computed_line,
            "line_start must match the real file line"
        );
        // "Deploys happen on Friday." is on line 22 of the onboarding wiki.
        assert_eq!(hit.line_start, 22);
    }

    #[test]
    fn segment_empty_input_is_empty() {
        assert!(segment_candidates("").is_empty());
        assert!(segment_candidates("   \n\n  \n").is_empty());
    }

    #[test]
    fn oversized_physical_line_is_bounded_without_hiding_following_prose() {
        let oversized = "x".repeat(MAX_EXTRACTABLE_LINE_BYTES + 1);
        let source = format!("{oversized}\n\nDeploys happen on Friday.\n");
        let doc = MarkdownDocument::from_bytes("long.md", source.as_bytes()).expect("document");
        assert_eq!(doc.lines.len(), 2);
        assert_eq!(doc.lines[0].number, 2);
        assert_eq!(doc.lines[1].number, 3);

        let spans = segment_candidates(&source);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "Deploys happen on Friday.");
        assert_eq!(spans[0].line_start, 3);
    }

    #[test]
    fn segment_handles_multibyte_chars_without_panic() {
        // Regression: a prose span ending just after a multi-byte char (em-dash,
        // 3 bytes) made line_of_offset slice a `str` at a non-char-boundary index
        // and panic. Real corpora are full of —, “smart quotes”, and é/ü.
        let source = "# Eng Sync — raw notes\n\nDecision: Bob owns release approval now — \
                      effective immediately. Héllo, ünïcode “world”.\n\nThe big one —\n";
        let spans = segment_candidates(source);
        // The decision prose must be captured, and every offset must slice back.
        let hit = spans
            .iter()
            .find(|s| s.text.contains("Bob owns release approval"))
            .expect("decision prose span must be emitted");
        assert_eq!(&source[hit.char_start..hit.char_end], hit.text);
        assert!(hit.line_start >= 1);
    }

    #[test]
    fn line_of_offset_inside_multibyte_char_does_not_panic() {
        // Directly exercise the offset->line helper at a byte index that lands
        // inside the em-dash (a non-char-boundary), which previously panicked.
        let source = "ab—cd\n"; // '—' occupies bytes 2..5
                                // Offsets 3 and 4 are inside the em-dash.
        assert_eq!(line_of_offset(source, 3), 1);
        assert_eq!(line_of_offset(source, 4), 1);
        assert_eq!(line_of_offset(source, source.len()), 2);
    }
}
