//! Claim extraction scaffolding.

/// Default heuristic confidence in parts per million.
pub const DEFAULT_CONFIDENCE_PPM: u32 = 500_000;

pub mod faithfulness;
pub mod cache;
pub mod heuristics;
pub mod hints;
pub mod llm;
pub mod markdown;
pub mod normalize;
pub mod word_match;
