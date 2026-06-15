//! Agent-facing projections.

pub mod context;
pub mod freshness;

pub use context::{
    build_agent_context, explain_claim, AgentClaim, AgentContext, AgentStaleClaim, ClaimExplanation,
};
pub use freshness::FreshnessView;
