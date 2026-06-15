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
        let source_id = source_id_from_hash(&body_hash_hex).to_string();
        let text =
            String::from_utf8(bytes.to_vec()).map_err(|e| SourceError::Utf8(e.to_string()))?;
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
    Utf8(String),
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

    #[test]
    fn skips_code_fence() {
        let doc = MarkdownDocument::from_bytes(
            "test.md",
            b"# Title\n\n```\ncode\n```\n\nDeploys happen on Friday.\n",
        )
        .expect("parse");
        assert_eq!(doc.lines.len(), 4);
        assert!(doc.lines.iter().all(|l| !l.text.contains("code")));
    }
}
