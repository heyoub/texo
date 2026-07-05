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

/// Failed to construct an [`EventIdHex`] from an untrusted string.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EventIdHexError {
    /// Input was empty (after any `0x` prefix).
    #[error("event id hex must not be empty")]
    Empty,
    /// Input contained a non-hex-digit character.
    #[error("event id hex contains a non-hex digit")]
    NotHex,
}

impl EventIdHex {
    /// Construct from a `0x`-prefixed or plain hex string, validating that the
    /// remaining characters are hex digits.
    ///
    /// Returns [`EventIdHexError`] for empty or non-hex input rather than
    /// silently producing a malformed id.
    pub fn new(value: impl Into<String>) -> Result<Self, EventIdHexError> {
        let value = value.into();
        let digits = value.strip_prefix("0x").unwrap_or(&value);
        if digits.is_empty() {
            return Err(EventIdHexError::Empty);
        }
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(EventIdHexError::NotHex);
        }
        Ok(Self(format!("0x{digits}")))
    }

    /// Construct from a numeric BatPak event id.
    ///
    /// Total: lower-hex formatting of a `u128` always yields valid `0x`-prefixed
    /// hex, so this constructor cannot fail.
    pub fn from_event_id(event_id: u128) -> Self {
        Self(format!("{event_id:#x}"))
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
    /// Build a view from a BatPak append receipt that was already verified
    /// against the store at write time.
    ///
    /// Note on scope: the `AppendReceipt` signing material (`key_id`,
    /// `signature`, `extensions`) is intentionally NOT carried onto the view.
    /// Those fields are ephemeral — BatPak's [`StoredEvent`]/`EventHeader` do
    /// not persist them — so they cannot be recovered at replay/verify time and
    /// would not round-trip through the JSON surface. Re-verification of a
    /// projected view (see [`crate::journal::receipt::verify_receipt_view`])
    /// therefore performs an existence + sequence + content-hash consistency
    /// check on the unsigned path; signed re-verification is not yet wired (see
    /// that function's docs).
    ///
    /// [`StoredEvent`]: batpak::prelude::StoredEvent
    pub fn from_verified_append(
        receipt: &AppendReceipt,
        kind: &str,
        scope: &str,
        entity: &str,
    ) -> Self {
        ReceiptView {
            event_id: EventIdHex::from_event_id(u128::from(receipt.event_id)),
            sequence: LocalSequence::new(receipt.global_sequence),
            kind: kind.to_string(),
            scope: scope.to_string(),
            entity: entity.to_string(),
        }
    }
}

/// Build a receipt view from bare BatPak append metadata.
///
/// Used by the replay path, which reconstructs a view from a persisted
/// [`StoredEvent`] (event id, sequence, coordinate). The original receipt's
/// signing material is not persisted by BatPak and so is not available here;
/// the produced view describes the unsigned, replay-recoverable shape.
///
/// [`StoredEvent`]: batpak::prelude::StoredEvent
pub fn receipt_view(
    event_id: u128,
    sequence: u64,
    kind: &str,
    scope: &str,
    entity: &str,
) -> ReceiptView {
    ReceiptView {
        event_id: EventIdHex::from_event_id(event_id),
        sequence: LocalSequence::new(sequence),
        kind: kind.to_string(),
        scope: scope.to_string(),
        entity: entity.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_id_hex_normalizes_plain_input_with_prefix() {
        // Plain hex gains a single `0x` prefix; it must not be double-prefixed.
        let id = EventIdHex::new("deadBEEF").expect("valid hex");
        assert_eq!(id.as_str(), "0xdeadBEEF");
        assert_eq!(id.to_string(), "0xdeadBEEF");
    }

    #[test]
    fn event_id_hex_preserves_already_prefixed_input() {
        let id = EventIdHex::new("0x01ab").expect("valid hex");
        assert_eq!(id.as_str(), "0x01ab");
    }

    #[test]
    fn event_id_hex_rejects_empty_input() {
        // Both a bare empty string and a `0x`-only string have no digits.
        assert_eq!(EventIdHex::new(""), Err(EventIdHexError::Empty));
        assert_eq!(EventIdHex::new("0x"), Err(EventIdHexError::Empty));
    }

    #[test]
    fn event_id_hex_rejects_non_hex_digits() {
        assert_eq!(EventIdHex::new("0xnothex"), Err(EventIdHexError::NotHex));
        assert_eq!(EventIdHex::new("ghij"), Err(EventIdHexError::NotHex));
    }

    #[test]
    fn event_id_hex_from_numeric_id_is_lower_hex_prefixed() {
        assert_eq!(EventIdHex::from_event_id(0).as_str(), "0x0");
        assert_eq!(EventIdHex::from_event_id(255).as_str(), "0xff");
    }

    #[test]
    fn receipt_view_carries_all_metadata() {
        let view = receipt_view(16, 7, "ClaimRecorded", "workspace:demo", "claim:abc");
        assert_eq!(view.event_id.as_str(), "0x10");
        assert_eq!(view.sequence.get(), 7);
        assert_eq!(view.kind, "ClaimRecorded");
        assert_eq!(view.scope, "workspace:demo");
        assert_eq!(view.entity, "claim:abc");
    }

    #[test]
    fn receipt_view_serde_round_trips() {
        // Golden surface guard: the JSON shape that agents/humans consume must
        // round-trip unchanged.
        let view = receipt_view(16, 7, "ClaimRecorded", "workspace:demo", "claim:abc");
        let json = serde_json::to_string(&view).expect("serialize");
        let back: ReceiptView = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(view, back);
    }
}
