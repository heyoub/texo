//! Markdown source parsing.

use std::path::{Path, PathBuf};

use crate::types::ids::{blake3_bytes_hex, source_id_from_hash};

/// One logical line in a markdown document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownLine {
    /// 1-based line number.
    pub number: u32,
    /// Raw line text.
    pub text: String,
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

    for (idx, raw) in text.lines().enumerate() {
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
        });
    }
    lines
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
    Id(#[from] crate::types::IdParseError),
}

/// Collect markdown files under a directory.
pub fn collect_markdown_files(input: &Path) -> Result<Vec<PathBuf>, SourceError> {
    let mut files = Vec::new();
    if input.is_file() {
        if is_markdown(input) {
            files.push(input.to_path_buf());
        }
        return Ok(files);
    }
    for entry in walkdir::WalkDir::new(input)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.is_file() && is_markdown(path) {
            files.push(path.to_path_buf());
        }
    }
    files.sort();
    Ok(files)
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
}
