//! Source ingestion helpers.

pub mod markdown;

pub use markdown::{collect_markdown_files, MarkdownDocument, MarkdownLine, SourceError};
