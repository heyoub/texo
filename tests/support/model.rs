//! Model-capability setup for integration tests that must not call transport.

use std::path::Path;

use texo::config::TexoRootConfig;
use texo::gateway::{GatewayConfig, ProviderProfile, DEFAULT_PROVIDER_ID};

/// Configure a model-capable test host without storing or mutating a secret.
pub fn write_model_capable_config(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut gateway = GatewayConfig::default();
    gateway.providers.insert(
        DEFAULT_PROVIDER_ID.to_string(),
        ProviderProfile {
            base_url: "https://model-transport-must-not-run.invalid/v1".to_string(),
            api_key_env: "PATH".to_string(),
            ..ProviderProfile::default()
        },
    );
    let mut config = TexoRootConfig::demo();
    config.gateway = Some(gateway);
    config.save(&root.join(".texo/config.toml"))?;
    Ok(())
}
