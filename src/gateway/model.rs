//! Gateway configuration shapes whose public names remain stable by re-export.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{ProviderProfile, RoleConfig};

/// Optional `[gateway]` configuration. Absence means built-in defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GatewayConfig {
    /// Named provider profiles.
    pub providers: BTreeMap<String, ProviderProfile>,
    /// Optional embedding-role override.
    pub embed: Option<RoleConfig>,
    /// Optional proposal-role override.
    pub propose: Option<RoleConfig>,
    /// Optional relation-role override.
    pub relate: Option<RoleConfig>,
    /// Optional chat-role override.
    pub chat: Option<RoleConfig>,
    /// Global semantic relate wall-clock budget.
    #[serde(default = "super::default_relate_budget_secs")]
    pub relate_budget_secs: u64,
}

/// Already-read neutral environment values used by the pure resolver.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatewayEnvironment {
    /// `TEXO_LLM_BASE_URL` value.
    pub base_url: Option<String>,
    /// Value read from the selected provider profile's key environment name.
    pub api_key: Option<String>,
    /// Role-specific `TEXO_LLM_*_MODEL` value.
    pub model: Option<String>,
}
