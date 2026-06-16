//! Source ingestion helpers.

pub mod markdown;

pub use markdown::{
    collect_markdown_files, segment_candidates, CandidateSpan, MarkdownDocument, MarkdownLine,
    SourceError,
};
