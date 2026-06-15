//! Line normalization for claim extraction.

/// Normalize a line for claim identity and comparison.
pub fn normalize_line(line: &str) -> String {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    lower.split_whitespace().collect::<Vec<_>>().join(" ")
}
