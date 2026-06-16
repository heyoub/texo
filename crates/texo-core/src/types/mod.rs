//! Shared domain types.

pub mod coordinate;
pub mod ids;
pub mod parse;
pub mod receipt;
pub mod sequence;
pub mod status;

pub use coordinate::{
    entity_for_claim, entity_for_conflict, entity_for_projection, entity_for_source,
    scope_for_workspace,
};

pub use ids::{
    blake3_bytes_hex, blake3_hash_hex, claim_id_from_parts, conflict_id_from_pair,
    source_id_from_hash, ClaimId, ConflictId, DocId, SourceId, WorkspaceId,
};
pub use parse::IdParseError;
pub use receipt::{receipt_view, EventIdHex, EventIdHexError, ReceiptView};
pub use sequence::{ConfidencePpm, InvalidConfidence, LocalSequence, ObservedAtMs, ReplayFrontier};
pub use status::{ClaimStatus, ConflictStatus};
