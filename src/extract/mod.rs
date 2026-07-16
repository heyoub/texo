//! Claim extraction scaffolding.

/// Default heuristic confidence in parts per million.
pub const DEFAULT_CONFIDENCE_PPM: u32 = 500_000;

pub mod cache;
/// Claim grounding and faithfulness assessment.
pub mod faithfulness;
pub mod heuristics;
/// Extracted claim hint surface.
pub mod hints;
pub mod llm;
/// Markdown discovery and segmentation.
pub mod markdown;
/// Claim text normalization.
pub mod normalize;
pub mod word_match;
