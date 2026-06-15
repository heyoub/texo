//! Branded numeric and sequence types.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Per-store commit order from BatPak append receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent, deny_unknown_fields)]
pub struct LocalSequence(u64);

impl LocalSequence {
    /// Construct from a raw store sequence value.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// Raw sequence number.
    pub fn get(self) -> u64 {
        self.0
    }

    /// Merge two sequences keeping the maximum (replay frontier).
    #[must_use]
    pub fn max(self, other: Self) -> Self {
        Self(self.0.max(other.0))
    }
}

impl fmt::Display for LocalSequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "local seq {}", self.0)
    }
}

/// Replay frontier — maximum local sequence observed during journal replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent, deny_unknown_fields)]
pub struct ReplayFrontier(LocalSequence);

impl ReplayFrontier {
    /// Zero frontier before any events.
    pub const ZERO: Self = Self(LocalSequence(0));

    /// Construct from a local sequence.
    pub fn new(sequence: LocalSequence) -> Self {
        Self(sequence)
    }

    /// Underlying sequence.
    pub fn sequence(self) -> LocalSequence {
        self.0
    }

    /// Advance frontier if `sequence` is greater.
    pub fn advance(&mut self, sequence: LocalSequence) {
        self.0 = self.0.max(sequence);
    }
}

impl fmt::Display for ReplayFrontier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "replayed through seq {}", self.0.get())
    }
}

/// Integer confidence from 0 to 1_000_000 parts per million.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent, deny_unknown_fields)]
pub struct ConfidencePpm(u32);

impl ConfidencePpm {
    /// Maximum confidence value.
    pub const MAX: u32 = 1_000_000;

    /// Construct validating the inclusive range.
    pub fn new(value: u32) -> Result<Self, InvalidConfidence> {
        if value > Self::MAX {
            return Err(InvalidConfidence(value));
        }
        Ok(Self(value))
    }

    /// Raw parts-per-million value.
    pub fn get(self) -> u32 {
        self.0
    }
}

/// Confidence outside the allowed range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidConfidence(pub u32);

/// Wall-clock observation time in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent, deny_unknown_fields)]
pub struct ObservedAtMs(u64);

impl ObservedAtMs {
    /// Construct from raw milliseconds.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// Raw milliseconds.
    pub fn get(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_rejects_out_of_range() {
        assert!(ConfidencePpm::new(1_000_001).is_err());
        assert_eq!(ConfidencePpm::new(900_000).expect("ppm").get(), 900_000);
    }
}
