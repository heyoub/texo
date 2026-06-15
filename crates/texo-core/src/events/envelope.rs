//! Typed event envelope for replay.

use serde::{Deserialize, Serialize};

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

/// Failure decoding a stored journal entry.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// Unknown or unsupported event kind.
    #[error("unsupported event kind")]
    UnsupportedKind,
    /// Payload bytes could not be decoded.
    #[error("decode failure: {0}")]
    Decode(String),
}

/// Serializable event summary for tests.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventSummary {
    /// Event kind.
    pub kind: String,
    /// Local sequence.
    pub sequence: u64,
}
