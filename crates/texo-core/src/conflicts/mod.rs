//! Conflict detection and commit.

pub mod commit;
pub mod detect;
pub mod verify;

pub use commit::commit_conflicts;
pub use detect::{detect_conflicts, verify_projection, verify_store, VerifyError};
pub use verify::verify_journal_receipts;
