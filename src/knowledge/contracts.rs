use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{AnalysisQuality, CoverageGap};

/// Failure to construct a knowledge contract value.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum KnowledgeContractError {
    /// A branded knowledge identifier was malformed.
    #[error("invalid {kind} identifier")]
    InvalidIdentifier {
        /// Identifier class being validated.
        kind: &'static str,
    },
    /// A digest was not lowercase hexadecimal with the required length.
    #[error("{field} must be exactly {length} lowercase hexadecimal characters")]
    InvalidDigest {
        /// Field being validated.
        field: &'static str,
        /// Required character length.
        length: usize,
    },
    /// A half-open byte range was reversed.
    #[error("byte range start {start} exceeds end {end}")]
    ReversedRange {
        /// Inclusive range start.
        start: u64,
        /// Exclusive range end.
        end: u64,
    },
    /// A line range was zero-based or reversed.
    #[error("line range must be one-based and ordered; received {start}..={end}")]
    InvalidLineRange {
        /// Inclusive first line.
        start: u32,
        /// Inclusive last line.
        end: u32,
    },
    /// An evidence excerpt exceeded its durable bound.
    #[error("evidence excerpt is {actual} bytes; maximum is {maximum}")]
    ExcerptTooLarge {
        /// Supplied byte length.
        actual: usize,
        /// Maximum byte length.
        maximum: usize,
    },
}

/// Coverage carried by every knowledge result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeCoverage {
    /// Strongest analysis quality actually used.
    pub analysis_quality: AnalysisQuality,
    /// Number of source items examined.
    pub sources_examined: u64,
    /// Number of evidence occurrences returned or recorded.
    pub occurrences: u64,
    /// Whether the operation stopped at a configured bound.
    pub truncated: bool,
    /// Typed omissions; empty only when no known gap exists.
    pub gaps: Vec<CoverageGap>,
}
