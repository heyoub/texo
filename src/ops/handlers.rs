//! Texo operation handlers, organized by operation concern.

use syncbat::{CoreBuilder, OperationRegisterItem};

mod agent_context;
mod claims;
mod common;
mod compile;
mod conflicts;
mod host;
mod ingest;
mod knowledge;
mod knowledge_read;
mod model;
mod relate;
mod render;
mod stats;
mod verify;
mod workspace;

pub(crate) use common::{
    append_json, assemble_current_view, op_runtime, parse_input, run_op, take_receipts,
    workspace_temporal_policy,
};
pub(crate) use ingest::{
    infer_supersessions, plan_sources, ExplicitSupersessionOutcome, SourcePlan,
};
pub use ingest::{ExplicitSupersessionHoldReason, HeldExplicitSupersession};
pub(crate) use relate::{run_relate_pass, RelatePassOptions};

struct OperationBinding {
    item: fn() -> OperationRegisterItem,
    register: for<'a> fn(&'a mut CoreBuilder) -> Result<&'a mut CoreBuilder, syncbat::BuildError>,
}

const OPERATIONS: &[OperationBinding] = &[
    OperationBinding {
        item: workspace::workspace_init_item,
        register: workspace::register_workspace_init,
    },
    OperationBinding {
        item: ingest::ingest_run_item,
        register: ingest::register_ingest_run,
    },
    OperationBinding {
        item: claims::claims_list_item,
        register: claims::register_claims_list,
    },
    OperationBinding {
        item: claims::claims_search_item,
        register: claims::register_claims_search,
    },
    OperationBinding {
        item: claims::knowledge_search_item,
        register: claims::register_knowledge_search,
    },
    OperationBinding {
        item: claims::claim_explain_item,
        register: claims::register_claim_explain,
    },
    OperationBinding {
        item: knowledge::knowledge_triangulate_item,
        register: knowledge::register_knowledge_triangulate,
    },
    OperationBinding {
        item: claims::claim_supersede_item,
        register: claims::register_claim_supersede,
    },
    OperationBinding {
        item: verify::verify_run_item,
        register: verify::register_verify_run,
    },
    OperationBinding {
        item: render::staleness_check_item,
        register: render::register_staleness_check,
    },
    OperationBinding {
        item: agent_context::context_agent_item,
        register: agent_context::register_context_agent,
    },
    OperationBinding {
        item: compile::compile_run_item,
        register: compile::register_compile_run,
    },
    OperationBinding {
        item: conflicts::conflicts_list_item,
        register: conflicts::register_conflicts_list,
    },
    OperationBinding {
        item: conflicts::conflicts_commit_item,
        register: conflicts::register_conflicts_commit,
    },
    OperationBinding {
        item: conflicts::conflict_resolve_item,
        register: conflicts::register_conflict_resolve,
    },
    OperationBinding {
        item: relate::relate_run_item,
        register: relate::register_relate_run,
    },
    OperationBinding {
        item: host::host_fingerprint_item,
        register: host::register_host_fingerprint,
    },
    OperationBinding {
        item: knowledge::knowledge_index_item,
        register: knowledge::register_knowledge_index,
    },
    OperationBinding {
        item: knowledge::code_index_build_item,
        register: knowledge::register_code_index_build,
    },
    OperationBinding {
        item: knowledge::knowledge_reconcile_item,
        register: knowledge::register_knowledge_reconcile,
    },
    OperationBinding {
        item: stats::stats_read_item,
        register: stats::register_stats_read,
    },
    OperationBinding {
        item: workspace::workspace_status_item,
        register: workspace::register_workspace_status,
    },
];

/// Return the operation catalog in deterministic registration order.
#[must_use]
pub fn catalog() -> Vec<OperationRegisterItem> {
    OPERATIONS
        .iter()
        .map(|operation| (operation.item)())
        .collect()
}

/// Register every operation handler.
///
/// # Errors
///
/// Returns the first registration error reported by syncbat.
pub fn register_all(builder: &mut CoreBuilder) -> Result<(), syncbat::BuildError> {
    for operation in OPERATIONS {
        (operation.register)(builder)?;
    }
    Ok(())
}
