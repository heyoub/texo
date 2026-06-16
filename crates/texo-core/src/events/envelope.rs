//! Typed event envelope for replay.

use super::payloads::{
    ClaimConflictDetected, ClaimRecorded, ClaimSuperseded, OnboardingCompiled, SourceObserved,
};
use crate::types::receipt::ReceiptView;
use crate::types::sequence::LocalSequence;

/// Decoded texo domain event with receipt metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TexoEvent {
    /// Source observed.
    SourceObserved {
        /// Payload.
        payload: SourceObserved,
        /// Receipt metadata.
        receipt: ReceiptView,
    },
    /// Claim recorded.
    ClaimRecorded {
        /// Payload.
        payload: ClaimRecorded,
        /// Receipt metadata.
        receipt: ReceiptView,
    },
    /// Claim superseded.
    ClaimSuperseded {
        /// Payload.
        payload: ClaimSuperseded,
        /// Receipt metadata.
        receipt: ReceiptView,
    },
    /// Conflict detected.
    ClaimConflictDetected {
        /// Payload.
        payload: ClaimConflictDetected,
        /// Receipt metadata.
        receipt: ReceiptView,
    },
    /// Onboarding compiled.
    OnboardingCompiled {
        /// Payload.
        payload: OnboardingCompiled,
        /// Receipt metadata.
        receipt: ReceiptView,
    },
}

impl TexoEvent {
    /// Local sequence for this event.
    pub fn sequence(&self) -> LocalSequence {
        match self {
            Self::SourceObserved { receipt, .. }
            | Self::ClaimRecorded { receipt, .. }
            | Self::ClaimSuperseded { receipt, .. }
            | Self::ClaimConflictDetected { receipt, .. }
            | Self::OnboardingCompiled { receipt, .. } => receipt.sequence,
        }
    }

    /// Kind label for logging and JSON.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SourceObserved { .. } => "SourceObserved",
            Self::ClaimRecorded { .. } => "ClaimRecorded",
            Self::ClaimSuperseded { .. } => "ClaimSuperseded",
            Self::ClaimConflictDetected { .. } => "ClaimConflictDetected",
            Self::OnboardingCompiled { .. } => "OnboardingCompiled",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::receipt::receipt_view;

    fn conflict_event() -> TexoEvent {
        TexoEvent::ClaimConflictDetected {
            payload: ClaimConflictDetected {
                conflict_id: "conflict_aaaaaaaaaaaa".to_string(),
                workspace_id: "demo".to_string(),
                claim_a: "claim_aaaaaaaaaaaa".to_string(),
                claim_b: "claim_bbbbbbbbbbbb".to_string(),
                reason: "test".to_string(),
                status: "open".to_string(),
                observed_at_ms: 1,
            },
            receipt: receipt_view(
                7,
                7,
                "ClaimConflictDetected",
                "workspace:demo",
                "conflict_aaaaaaaaaaaa",
            ),
        }
    }

    #[test]
    fn conflict_event_kind_and_sequence() {
        // The ClaimConflictDetected variant must report its kind label and its
        // receipt sequence (covers the conflict arms of `kind()` and
        // `sequence()`).
        let event = conflict_event();
        assert_eq!(event.kind(), "ClaimConflictDetected");
        assert_eq!(event.sequence().get(), 7);
    }
}

/// Failure decoding a stored journal entry.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// Unknown or unsupported event kind.
    #[error("unsupported event kind")]
    UnsupportedKind,
    /// Underlying store read failed while fetching the entry to decode.
    #[error("decode failure: {0}")]
    Store(#[from] batpak::prelude::StoreError),
    /// Typed payload decode failed.
    #[error("decode failure: {0}")]
    Decode(#[from] batpak::prelude::TypedDecodeError),
}
