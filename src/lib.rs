//! texo: claim-chain memory on the batpak family.

/// Curated agent-facing tools and operation discovery.
pub mod agent_catalog;
/// Evidence-backed workspace backup and offline verification.
pub mod backup;

/// Composed config, store, gateway, and integration diagnostics.
pub mod doctor;

/// Error types.
pub mod error;

pub mod lexicon;
/// Typed configuration and resolution for every model role.
pub mod gateway;
/// Deterministic Git commit and worktree source capture.
pub mod git_source;
/// Fixed read-only agent hook contracts.
pub mod hooks;
/// Idempotent agent-appliance installation.
pub mod install;
/// Read/write authority wrapper over BatPak store states.
pub mod journal_store;
/// Snapshot-consistent evidence and code-knowledge contracts.
pub mod knowledge;
/// Configuration loading and defaults.
pub mod config;
/// Event identifiers and schema surfaces.
pub mod events;
/// Claim-domain scaffolding.
pub mod claims;
/// Narrow replaceable adapters over BatPak family surfaces.
pub mod compat;
/// Bounded SCIP/syntactic/lexical code intelligence and disposable artifacts.
pub mod code_index;
/// Context assembly scaffolding.
pub mod context;
/// Claim extraction.
pub mod extract;
/// Semantic relation traits and pipeline.
pub mod semantics;
/// Relation orchestration scaffolding.
pub mod relate;
/// Proposal-only semantic reconciliation between claims and code evidence.
pub mod reconcile;
/// Exact-fork and imported-read-model replica circuits.
pub mod replication;
/// Operation scaffolding.
pub mod ops;
/// Host integration scaffolding.
pub mod host;
/// User-facing surfaces.
pub mod surfaces;
/// Explicit multi-journal workspace topology and replica roles.
pub mod topology;
