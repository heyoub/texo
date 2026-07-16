use std::path::{Component, Path};

use crate::error::TexoError;
use crate::knowledge::{ByteRange, CodeOccurrence, LineRange, MAX_EVIDENCE_EXCERPT_BYTES};

pub(super) fn source_context(
    bytes: &[u8],
    occurrence_start: usize,
    occurrence_end: usize,
) -> Option<(String, ByteRange, LineRange)> {
    if occurrence_start > occurrence_end || occurrence_end > bytes.len() {
        return None;
    }
    let line_start = bytes[..occurrence_start]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position.saturating_add(1));
    let line_end = bytes[occurrence_end..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |position| {
            occurrence_end.saturating_add(position)
        });
    let occurrence_len = occurrence_end.saturating_sub(occurrence_start);
    if occurrence_len > MAX_EVIDENCE_EXCERPT_BYTES {
        return None;
    }
    let available = MAX_EVIDENCE_EXCERPT_BYTES.saturating_sub(occurrence_len);
    let before = available / 2;
    let mut start = occurrence_start.saturating_sub(before).max(line_start);
    let mut end = occurrence_end
        .saturating_add(available.saturating_sub(occurrence_start.saturating_sub(start)))
        .min(line_end);
    if end.saturating_sub(start) < MAX_EVIDENCE_EXCERPT_BYTES {
        start = end
            .saturating_sub(MAX_EVIDENCE_EXCERPT_BYTES)
            .max(line_start);
    }
    while start < occurrence_start && bytes.get(start).is_some_and(|byte| byte & 0xc0 == 0x80) {
        start = start.saturating_add(1);
    }
    while end > occurrence_end && bytes.get(end).is_some_and(|byte| byte & 0xc0 == 0x80) {
        end = end.saturating_sub(1);
    }
    let context = std::str::from_utf8(&bytes[start..end]).ok()?.to_string();
    let (start_line, end_line) = byte_line_range(bytes, start, end);
    Some((
        context,
        ByteRange {
            start: u64::try_from(start).unwrap_or(u64::MAX),
            end: u64::try_from(end).unwrap_or(u64::MAX),
        },
        LineRange {
            start: start_line,
            end: end_line,
        },
    ))
}

pub(super) fn byte_line_range(bytes: &[u8], start: usize, end: usize) -> (u32, u32) {
    let start_line =
        1_u32.saturating_add(u32::try_from(count_newlines(&bytes[..start])).unwrap_or(u32::MAX));
    let end_line = start_line
        .saturating_add(u32::try_from(count_newlines(&bytes[start..end])).unwrap_or(u32::MAX));
    (start_line, end_line)
}

pub(super) fn count_newlines(bytes: &[u8]) -> usize {
    let mut count = 0_usize;
    for byte in bytes {
        if *byte == b'\n' {
            count = count.saturating_add(1);
        }
    }
    count
}

pub(super) fn code_occurrence_order(
    left: &CodeOccurrence,
    right: &CodeOccurrence,
) -> std::cmp::Ordering {
    left.symbol
        .cmp(&right.symbol)
        .then_with(|| left.path.cmp(&right.path))
        .then_with(|| left.byte_range.start.cmp(&right.byte_range.start))
        .then_with(|| left.byte_range.end.cmp(&right.byte_range.end))
        .then_with(|| left.roles.cmp(&right.roles))
}

pub(super) fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !Path::new(path).is_absolute()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

pub(super) fn source_error(path: &Path, detail: &str) -> TexoError {
    TexoError::Source {
        path: path.display().to_string(),
        detail: detail.to_string(),
    }
}
