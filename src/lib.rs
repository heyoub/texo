//! texo: claim-chain memory on the batpak family.

/// Error types.
pub mod error;
/// Typed configuration and resolution for every model role.
pub mod gateway;
/// Configuration loading and defaults.
pub mod config;
/// Event identifiers and schema surfaces.
pub mod events;
/// Claim-domain scaffolding.
pub mod claims;
/// Context assembly scaffolding.
pub mod context;
/// Claim extraction.
pub mod extract;
/// Semantic relation traits and pipeline.
pub mod semantics;
/// Relation orchestration scaffolding.
pub mod relate;
/// Operation scaffolding.
pub mod ops;
/// Host integration scaffolding.
pub mod host;
/// User-facing surfaces.
pub mod surfaces;
