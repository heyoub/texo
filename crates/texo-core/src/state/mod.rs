//! State machines and typestate wrappers.

pub mod claim_lifecycle;
pub mod conflict_lifecycle;
pub mod ingest;
pub mod journal;

pub use claim_lifecycle::{
    apply_open_conflict, apply_supersession, initial_claim_status, transition,
    validate_supersession, ClaimLifecycleEvent, TransitionError,
};
pub use conflict_lifecycle::{CommittedConflict, ConflictEntry, ConflictReport};
pub use ingest::{IngestCommitted, IngestMode, IngestPlan, IngestReport};
pub use journal::{Closed, Journal, JournalState, Open};
