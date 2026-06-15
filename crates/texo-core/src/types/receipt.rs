//! Receipt view types for JSON, CLI, and MCP surfaces.
//!
//! BatPak `AppendReceipt` is the source of truth at write time; `ReceiptView` is
//! the stable projection surfaced to agents and humans.

use std::fmt;

use batpak::prelude::AppendReceipt;
use serde::{Deserialize, Serialize};

use crate::types::sequence::LocalSequence;

/// Hex-encoded BatPak event id for JSON surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent, deny_unknown_fields)]
pub struct EventIdHex(String);

impl EventIdHex {
    /// Construct from a `0x`-prefixed or plain hex string.
    pub fn new(value: impl Into<String>) -> Self {
        let mut value = value.into();
        if !value.starts_with("0x") {
            value = format!("0x{value}");
        }
        Self(value)
    }

    /// Borrow hex representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EventIdHex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Append receipt surfaced to CLI, MCP, and JSON artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptView {
    /// Event identifier hex.
    pub event_id: EventIdHex,
    /// Local store commit sequence.
    pub sequence: LocalSequence,
    /// Event kind name.
    pub kind: String,
    /// BatPak coordinate scope.
    pub scope: String,
    /// BatPak coordinate entity.
    pub entity: String,
}

impl ReceiptView {
    /// Build from a verified BatPak append receipt.
    pub fn from_verified_append(
        receipt: &AppendReceipt,
        kind: &str,
        scope: &str,
        entity: &str,
    ) -> Self {
        receipt_view(
            receipt.event_id.into(),
            receipt.sequence,
            kind,
            scope,
            entity,
        )
    }
}

/// Build receipt view from BatPak append metadata.
pub fn receipt_view(
    event_id: u128,
    sequence: u64,
    kind: &str,
    scope: &str,
    entity: &str,
) -> ReceiptView {
    ReceiptView {
        event_id: EventIdHex::new(format!("{event_id:#x}")),
        sequence: LocalSequence::new(sequence),
        kind: kind.to_string(),
        scope: scope.to_string(),
        entity: entity.to_string(),
    }
}
