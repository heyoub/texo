//! Bounded SCIP import and built-in code-intelligence fallbacks.

/// Normalized artifact schema.
pub const ARTIFACT_SCHEMA: &str = "texo.code-index.v3";

mod contract;

pub use contract::{CodeIndexLimits, PreparedCodeIndex};

mod analysis;
mod artifact;
mod persistence;
mod scip_import;
mod util;

pub use artifact::build;
pub use persistence::{load, persist, read_scip};
