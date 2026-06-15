//! Human-readable artifact rendering.

pub mod html;
pub mod json;
pub mod markdown;

pub use html::{compile_artifacts, render_index_html, CompileOutput};
pub use json::{render_agent_json, render_claims_json, render_conflicts_json, render_stale_json};
pub use markdown::render_onboarding;
