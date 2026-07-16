//! Line normalization for claim extraction.

/// Normalize a line for claim identity and comparison.
#[must_use]
pub fn normalize_line(line: &str) -> String {
    let lower = line.trim().to_ascii_lowercase();
    let mut out = String::with_capacity(lower.len());
    for word in lower.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}
