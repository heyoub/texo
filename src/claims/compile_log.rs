//! Compile and workspace metadata projections.

use batpak::event::RawMsgpackInput;
use serde::{Deserialize, Serialize};

use crate::events::payloads::{OnboardingCompiledV2, WorkspaceInitializedV2};

/// One onboarding compile entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileEntry {
    /// Output path relative to the workspace root.
    pub output_path: String,
    /// Journal sequence replayed through for the compile.
    pub replayed_through_sequence: u64,
    /// Compile wall-clock time in milliseconds.
    pub compiled_at_ms: u64,
}

/// Compile log projection for onboarding artifacts.
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventSourced)]
#[batpak(input = RawMsgpackInput, cache_version = 1, state_max_cardinality = 1)]
#[batpak(event = OnboardingCompiledV2, handler = on_compiled)]
pub struct CompileLog {
    /// Compile entries in replay order.
    pub compiles: Vec<CompileEntry>,
}

impl CompileLog {
    fn on_compiled(&mut self, event: &OnboardingCompiledV2) {
        self.compiles.push(CompileEntry {
            output_path: event.output_path.clone(),
            replayed_through_sequence: event.replayed_through_sequence,
            compiled_at_ms: event.compiled_at_ms,
        });
    }
}

/// Workspace metadata projection.
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventSourced)]
#[batpak(input = RawMsgpackInput, cache_version = 1, state_max_cardinality = 1)]
#[batpak(event = WorkspaceInitializedV2, handler = on_initialized)]
pub struct WorkspaceCard {
    /// Workspace scope identifier.
    pub workspace_id: String,
    /// Schema identifier.
    pub schema: String,
    /// Configuration digest as lowercase hex.
    pub config_digest_hex: String,
    /// Creation wall-clock time in milliseconds.
    pub created_at_ms: u64,
}

impl WorkspaceCard {
    fn on_initialized(&mut self, event: &WorkspaceInitializedV2) {
        self.workspace_id.clone_from(&event.workspace_id);
        self.schema.clone_from(&event.schema);
        self.config_digest_hex.clone_from(&event.config_digest_hex);
        self.created_at_ms = event.created_at_ms;
    }
}
