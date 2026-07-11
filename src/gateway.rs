//! Typed model-gateway configuration and role resolution.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Built-in provider identifier and profile.
pub const DEFAULT_PROVIDER_ID: &str = "openrouter";
/// Neutral environment variable for the OpenAI-compatible base URL.
pub const ENV_BASE_URL: &str = "TEXO_LLM_BASE_URL";
/// Neutral environment variable holding the model API key.
pub const ENV_API_KEY: &str = "TEXO_LLM_API_KEY";
/// Neutral embedding-model environment variable.
pub const ENV_EMBED_MODEL: &str = "TEXO_LLM_EMBED_MODEL";
/// Neutral proposer-model environment variable.
pub const ENV_PROPOSE_MODEL: &str = "TEXO_LLM_PROPOSE_MODEL";
/// Neutral relation-model environment variable.
pub const ENV_RELATE_MODEL: &str = "TEXO_LLM_RELATE_MODEL";
/// Neutral chat-model environment variable.
pub const ENV_CHAT_MODEL: &str = "TEXO_LLM_CHAT_MODEL";

const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
const DEFAULT_EMBED_MODEL: &str = "google/gemini-embedding-2";
const DEFAULT_PROPOSE_MODEL: &str = "anthropic/claude-opus-4.8";
const DEFAULT_RELATE_MODEL: &str = "nvidia/nemotron-3-ultra-550b-a55b";
const DEFAULT_CHAT_MODEL: &str = "anthropic/claude-opus-4.8";

fn default_api_key_env() -> String {
    ENV_API_KEY.to_string()
}

fn default_base_url() -> String {
    DEFAULT_BASE_URL.to_string()
}

fn default_embed_batch_max() -> usize {
    64
}

fn default_retry_max() -> u32 {
    4
}

fn default_timeout_secs() -> u64 {
    120
}

/// Closed set of model responsibilities supported by Texo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    /// Vector embeddings for candidate discovery.
    Embed,
    /// Atomic claim proposal.
    Propose,
    /// Pairwise claim-relation judgment.
    Relate,
    /// Memory-grounded assistant chat.
    Chat,
}

impl ModelRole {
    /// Return the neutral model environment variable for this role.
    #[must_use]
    pub const fn model_env(self) -> &'static str {
        match self {
            Self::Embed => ENV_EMBED_MODEL,
            Self::Propose => ENV_PROPOSE_MODEL,
            Self::Relate => ENV_RELATE_MODEL,
            Self::Chat => ENV_CHAT_MODEL,
        }
    }
}

/// Provider-level transport and capability configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderProfile {
    /// OpenAI-compatible API base URL.
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// Name of the environment variable containing the secret API key.
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
    /// Maximum inputs sent in one embeddings request.
    #[serde(default = "default_embed_batch_max")]
    pub embed_batch_max: usize,
    /// Whether this provider accepts strict JSON-schema response formats.
    pub strict_json_schema_ok: bool,
    /// Whether completion tokens may be consumed by hidden reasoning.
    pub expects_reasoning: bool,
    /// Maximum number of retries after the initial request.
    #[serde(default = "default_retry_max")]
    pub retry_max: u32,
    /// Total request deadline in seconds.
    #[serde(default = "default_timeout_secs")]
    pub request_timeout_secs: u64,
}

impl Default for ProviderProfile {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            api_key_env: default_api_key_env(),
            embed_batch_max: default_embed_batch_max(),
            strict_json_schema_ok: true,
            expects_reasoning: true,
            retry_max: default_retry_max(),
            request_timeout_secs: default_timeout_secs(),
        }
    }
}

/// Provider response-format policy for a model role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormatPolicy {
    /// Prompt-only JSON; do not send `response_format`.
    None,
    /// Send the role's strict JSON schema when the provider supports it.
    JsonSchema,
}

/// Complete configuration for one model role.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleConfig {
    /// Provider profile id.
    pub provider: String,
    /// Provider model id.
    pub model: String,
    /// Completion-token ceiling.
    pub max_completion_tokens: u32,
    /// Sampling temperature.
    pub temperature: f32,
    /// Structured response policy.
    pub response_format: ResponseFormatPolicy,
}

/// Optional `[gateway]` configuration. Absence means built-in defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
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
}

impl GatewayConfig {
    fn role(&self, role: ModelRole) -> Option<&RoleConfig> {
        match role {
            ModelRole::Embed => self.embed.as_ref(),
            ModelRole::Propose => self.propose.as_ref(),
            ModelRole::Relate => self.relate.as_ref(),
            ModelRole::Chat => self.chat.as_ref(),
        }
    }
}

/// Explicit per-invocation overrides. Non-blank values take highest precedence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleOverrides {
    /// Base URL override.
    pub base_url: Option<String>,
    /// Secret API-key override.
    pub api_key: Option<String>,
    /// Model override.
    pub model: Option<String>,
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

/// Resolved provider and role configuration, including the secret key.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRole {
    /// Closed role being resolved.
    pub role: ModelRole,
    /// Provider identity used in provenance fingerprints.
    pub provider_id: String,
    /// Provider transport/capability profile.
    pub profile: ProviderProfile,
    /// Role-specific model and generation settings.
    pub config: RoleConfig,
    /// Resolved secret API key.
    pub api_key: String,
}

impl ResolvedRole {
    /// Whether this role has a usable secret key.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        !self.api_key.trim().is_empty()
    }
}

/// Resolve one role with `explicit > TEXO_LLM_* > TOML > defaults` precedence.
///
/// Environment reads are isolated here so adapters never carry their own
/// provider folklore or duplicate configuration sources.
#[must_use]
pub fn resolve_role(
    role: ModelRole,
    explicit: &RoleOverrides,
    gateway: Option<&GatewayConfig>,
) -> ResolvedRole {
    let key_env = selected_profile(role, gateway).api_key_env;
    resolve_role_with_environment(
        role,
        explicit,
        gateway,
        &GatewayEnvironment {
            base_url: std::env::var(ENV_BASE_URL).ok(),
            api_key: std::env::var(key_env).ok(),
            model: std::env::var(role.model_env()).ok(),
        },
    )
}

/// Pure role resolution over already-read environment values.
#[must_use]
pub fn resolve_role_with_environment(
    role: ModelRole,
    explicit: &RoleOverrides,
    gateway: Option<&GatewayConfig>,
    environment: &GatewayEnvironment,
) -> ResolvedRole {
    let default_role = default_role(role);
    let configured_role = gateway.and_then(|config| config.role(role));
    let provider_id = configured_role.map_or_else(
        || default_role.provider.clone(),
        |config| config.provider.clone(),
    );
    let mut profile = gateway
        .and_then(|config| config.providers.get(&provider_id))
        .cloned()
        .unwrap_or_default();
    profile.base_url = first_non_blank([
        explicit.base_url.clone(),
        environment.base_url.clone(),
        Some(profile.base_url.clone()),
    ])
    .unwrap_or_else(default_base_url)
    .trim_end_matches('/')
    .to_string();

    let configured_model = configured_role.map(|config| config.model.clone());
    let model = first_non_blank([
        explicit.model.clone(),
        environment.model.clone(),
        configured_model,
        Some(default_role.model.clone()),
    ])
    .unwrap_or_else(|| default_role.model.clone());
    let api_key = first_non_blank([explicit.api_key.clone(), environment.api_key.clone()])
        .unwrap_or_default();

    let mut config = configured_role.cloned().unwrap_or(default_role);
    config.provider.clone_from(&provider_id);
    config.model = model;
    ResolvedRole {
        role,
        provider_id,
        profile,
        config,
        api_key,
    }
}

fn selected_profile(role: ModelRole, gateway: Option<&GatewayConfig>) -> ProviderProfile {
    let default_role = default_role(role);
    let provider_id = gateway
        .and_then(|config| config.role(role))
        .map_or(default_role.provider, |config| config.provider.clone());
    gateway
        .and_then(|config| config.providers.get(&provider_id))
        .cloned()
        .unwrap_or_default()
}

fn first_non_blank<const N: usize>(values: [Option<String>; N]) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
}

fn default_role(role: ModelRole) -> RoleConfig {
    let (model, max_completion_tokens, response_format) = match role {
        ModelRole::Embed => (DEFAULT_EMBED_MODEL, 0, ResponseFormatPolicy::None),
        ModelRole::Propose => (DEFAULT_PROPOSE_MODEL, 2048, ResponseFormatPolicy::None),
        ModelRole::Relate => (DEFAULT_RELATE_MODEL, 4096, ResponseFormatPolicy::JsonSchema),
        ModelRole::Chat => (DEFAULT_CHAT_MODEL, 1024, ResponseFormatPolicy::None),
    };
    RoleConfig {
        provider: DEFAULT_PROVIDER_ID.to_string(),
        model: model.to_string(),
        max_completion_tokens,
        temperature: 0.0,
        response_format,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_role_environment_names_are_neutral() {
        assert_eq!(ModelRole::Embed.model_env(), "TEXO_LLM_EMBED_MODEL");
        assert_eq!(ModelRole::Propose.model_env(), "TEXO_LLM_PROPOSE_MODEL");
        assert_eq!(ModelRole::Relate.model_env(), "TEXO_LLM_RELATE_MODEL");
        assert_eq!(ModelRole::Chat.model_env(), "TEXO_LLM_CHAT_MODEL");
    }

    #[test]
    fn precedence_is_explicit_then_env_then_toml_then_default() {
        let mut gateway = GatewayConfig {
            relate: Some(RoleConfig {
                provider: DEFAULT_PROVIDER_ID.to_string(),
                model: "toml-model".to_string(),
                max_completion_tokens: 99,
                temperature: 0.25,
                response_format: ResponseFormatPolicy::None,
            }),
            ..GatewayConfig::default()
        };
        gateway.providers.insert(
            DEFAULT_PROVIDER_ID.to_string(),
            ProviderProfile {
                base_url: "https://toml.invalid/v1".to_string(),
                ..ProviderProfile::default()
            },
        );
        let resolved = resolve_role_with_environment(
            ModelRole::Relate,
            &RoleOverrides {
                base_url: Some("https://explicit.invalid/v1/".to_string()),
                api_key: Some("explicit-key".to_string()),
                model: Some("explicit-model".to_string()),
            },
            Some(&gateway),
            &GatewayEnvironment {
                base_url: Some("https://env.invalid/v1".to_string()),
                api_key: Some("env-key".to_string()),
                model: Some("env-model".to_string()),
            },
        );
        assert_eq!(resolved.profile.base_url, "https://explicit.invalid/v1");
        assert_eq!(resolved.api_key, "explicit-key");
        assert_eq!(resolved.config.model, "explicit-model");
        assert_eq!(resolved.config.max_completion_tokens, 99);

        let environment_wins = resolve_role_with_environment(
            ModelRole::Relate,
            &RoleOverrides::default(),
            Some(&gateway),
            &GatewayEnvironment {
                base_url: Some("https://env.invalid/v1".to_string()),
                api_key: Some("env-key".to_string()),
                model: Some("env-model".to_string()),
            },
        );
        assert_eq!(environment_wins.profile.base_url, "https://env.invalid/v1");
        assert_eq!(environment_wins.api_key, "env-key");
        assert_eq!(environment_wins.config.model, "env-model");

        let toml_wins = resolve_role_with_environment(
            ModelRole::Relate,
            &RoleOverrides::default(),
            Some(&gateway),
            &GatewayEnvironment::default(),
        );
        assert_eq!(toml_wins.profile.base_url, "https://toml.invalid/v1");
        assert_eq!(toml_wins.config.model, "toml-model");
    }
}
