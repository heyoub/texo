//! Journal adapter over BatPak.

pub mod append;
pub mod receipt;
pub mod replay;
pub mod store;

pub use store::{ingest_sources, plan_ingest_sources, JournalError, StoreHandle};
