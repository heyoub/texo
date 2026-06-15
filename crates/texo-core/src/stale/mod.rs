//! Staleness detection.

pub mod check;
pub mod diagnostic;

pub use check::{check_staleness, infer_supersessions};
pub use diagnostic::{StaleDiagnostic, StalenessReport};
