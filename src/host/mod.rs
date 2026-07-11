//! Host composition for texo operations.

pub mod fingerprint;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use batpak::coordinate::Coordinate;
use batpak::store::{Open, Store, StoreConfig};
use serde::{Deserialize, Serialize};
use syncbat::{Core, RuntimeError, StoreOperationStatusSink, StoreReceiptSink};

use crate::claims::workspace::WorkspaceCache;
use crate::config::{ConfigError, TexoRootConfig, WorkspaceConfig, WorkspaceEntry};
use crate::error::TexoError;
use crate::gateway::{resolve_role, ModelRole, RoleOverrides};
use crate::ops::backend::TexoEffectBackend;
use crate::ops::env::{self, OpEnv};
use crate::ops::{catalog, register_all};

/// Cross-request checkout slot for the warm workspace projection.
pub type SharedWorkspaceCache = Arc<Mutex<Option<WorkspaceCache>>>;

/// Deterministic fingerprints exposed by the composed texo host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostFingerprints {
    /// Digest of the texo operation module catalog.
    pub module_digest: String,
    /// Digest of the runnable host composition.
    pub host_fingerprint: String,
    /// Digest of the client-visible operation interface.
    pub interface_fingerprint: String,
}

/// Runnable texo host over a syncbat core.
pub struct TexoHost {
    core: Core,
    env: Rc<OpEnv>,
    fingerprints: HostFingerprints,
    shared_cache: Option<SharedWorkspaceCache>,
}

/// Open the configured workspace store.
///
/// # Errors
/// Returns [`TexoError::Config`] when `.texo/config.toml` cannot be loaded or
/// the workspace id cannot be resolved; [`TexoError::Registry`] when `BatPak`'s
/// payload registry is invalid; [`TexoError::Store`] when the store cannot be
/// opened.
pub fn open_workspace_store(
    root: impl AsRef<Path>,
    workspace_id: &str,
) -> Result<Arc<Store<Open>>, TexoError> {
    let root = root.as_ref();
    let config_path = root.join(".texo").join("config.toml");
    let root_config = TexoRootConfig::load(&config_path).map_err(config_error)?;
    let config = root_config
        .resolve(Some(workspace_id))
        .map_err(config_error)?;
    open_store_for_config(root, &config)
}

impl TexoHost {
    /// Build a runnable host for one workspace.
    ///
    /// # Errors
    /// Returns [`TexoError::Registry`] when `BatPak`'s payload registry is invalid;
    /// [`TexoError::Store`] when the store cannot be opened;
    /// [`TexoError::Host`] when syncbat registration or build validation fails.
    pub fn open(
        root: impl Into<PathBuf>,
        workspace_id: impl Into<String>,
        observed_at_ms: u64,
    ) -> Result<Self, TexoError> {
        let root = root.into();
        let workspace_id = workspace_id.into();
        let config = load_or_default_config(&root, &workspace_id)?;
        let store = open_store_for_config(&root, &config)?;
        Self::from_parts(root, &workspace_id, observed_at_ms, config, store, None)
    }

    /// Build a runnable host for `texo.workspace.init`.
    ///
    /// This opener permits the target workspace to be absent from an existing
    /// root config so init can add it.
    ///
    /// # Errors
    /// Returns [`TexoError::Registry`] when `BatPak`'s payload registry is invalid;
    /// [`TexoError::Store`] when the target store cannot be opened;
    /// [`TexoError::Host`] when syncbat registration or build validation fails.
    pub fn open_for_init(
        root: impl Into<PathBuf>,
        workspace_id: impl Into<String>,
        observed_at_ms: u64,
    ) -> Result<Self, TexoError> {
        let root = root.into();
        let workspace_id = workspace_id.into();
        let config = load_or_init_config(&root, &workspace_id)?;
        let store = open_store_for_config(&root, &config)?;
        Self::from_parts(root, &workspace_id, observed_at_ms, config, store, None)
    }

    /// Build a runnable host over an already-open workspace store.
    ///
    /// # Errors
    /// Returns [`TexoError::Registry`] when `BatPak`'s payload registry is invalid;
    /// [`TexoError::Host`] when syncbat registration or build validation fails.
    pub fn open_with_store(
        root: impl Into<PathBuf>,
        workspace_id: impl Into<String>,
        observed_at_ms: u64,
        store: Arc<Store<Open>>,
    ) -> Result<Self, TexoError> {
        let root = root.into();
        let workspace_id = workspace_id.into();
        batpak::event::validate_event_payload_registry().map_err(|error| TexoError::Registry {
            detail: error.to_string(),
        })?;
        let config = load_or_default_config(&root, &workspace_id)?;
        Self::from_parts(root, &workspace_id, observed_at_ms, config, store, None)
    }

    /// Build a host that checks out a shared warm projection for one request.
    ///
    /// # Errors
    /// Returns the same composition failures as [`Self::open_with_store`].
    pub fn open_with_store_and_cache(
        root: impl Into<PathBuf>,
        workspace_id: impl Into<String>,
        observed_at_ms: u64,
        store: Arc<Store<Open>>,
        shared_cache: SharedWorkspaceCache,
    ) -> Result<Self, TexoError> {
        let root = root.into();
        let workspace_id = workspace_id.into();
        batpak::event::validate_event_payload_registry().map_err(|error| TexoError::Registry {
            detail: error.to_string(),
        })?;
        let config = load_or_default_config(&root, &workspace_id)?;
        Self::from_parts(
            root,
            &workspace_id,
            observed_at_ms,
            config,
            store,
            Some(shared_cache),
        )
    }

    fn from_parts(
        root: PathBuf,
        workspace_id: &str,
        observed_at_ms: u64,
        config: WorkspaceConfig,
        store: Arc<Store<Open>>,
        shared_cache: Option<SharedWorkspaceCache>,
    ) -> Result<Self, TexoError> {
        let receipt_coordinate = op_receipt_coordinate(workspace_id)?;
        let receipt_sink = StoreReceiptSink::new(Arc::clone(&store), receipt_coordinate);
        let status_sink = StoreOperationStatusSink::new(Arc::clone(&store));

        let mut builder = Core::builder();
        register_all(&mut builder).map_err(build_error)?;
        builder.receipt_sink(receipt_sink);
        builder.status_sink(status_sink);
        builder.effect_backend(TexoEffectBackend);
        let model_role = resolve_role(
            ModelRole::Relate,
            &RoleOverrides::default(),
            config.gateway.as_ref(),
        );
        if grants_model_capability(Some(model_role.api_key)) {
            builder.grant_capability("texo.cap.model");
        }
        let core = builder.build().map_err(build_error)?;
        let cache = shared_cache
            .as_ref()
            .and_then(|slot| slot.lock().ok()?.take())
            .unwrap_or_else(|| WorkspaceCache::load(&root, workspace_id));
        let env = Rc::new(OpEnv {
            store,
            workspace_id: workspace_id.to_string(),
            root,
            config,
            cache: std::cell::RefCell::new(cache),
            receipts: std::cell::RefCell::new(Vec::new()),
            observed_at_ms,
        });
        let fingerprints = compute_fingerprints()?;
        Ok(Self {
            core,
            env,
            fingerprints,
            shared_cache,
        })
    }

    /// Invoke an operation with JSON input and JSON output.
    ///
    /// # Errors
    /// Returns [`TexoError::Json`] when input or output JSON cannot be encoded
    /// or decoded; [`TexoError::OpRuntime`] when syncbat denies or fails the
    /// operation.
    pub fn invoke_json(
        &mut self,
        op: &str,
        input: &serde_json::Value,
    ) -> Result<serde_json::Value, TexoError> {
        let bytes = serde_json::to_vec(input)?;
        self.env.receipts.borrow_mut().clear();
        let _guard = env::install(Rc::clone(&self.env));
        let result = self.core.invoke(op, bytes).map_err(runtime_error)?;
        serde_json::from_slice(result.output()).map_err(TexoError::Json)
    }

    /// Return deterministic host fingerprints.
    #[must_use]
    pub fn fingerprints(&self) -> HostFingerprints {
        self.fingerprints.clone()
    }

    /// Return the underlying store for tests and surfaces that need reads.
    #[must_use]
    pub fn store(&self) -> Arc<Store<Open>> {
        Arc::clone(&self.env.store)
    }

    /// Return the workspace id this host runs against.
    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.env.workspace_id
    }

    /// Return the root path this host runs against.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.env.root
    }

    /// Return the operation receipt coordinate used by this host.
    ///
    /// # Errors
    /// Returns [`TexoError::Coordinate`] if the deterministic coordinate fails
    /// `BatPak` validation.
    pub fn receipt_coordinate(&self) -> Result<Coordinate, TexoError> {
        op_receipt_coordinate(&self.env.workspace_id)
    }
}

impl Drop for TexoHost {
    fn drop(&mut self) {
        let cache = self.env.cache.borrow().clone();
        if let Err(error) = cache.save(&self.env.root, &self.env.workspace_id) {
            tracing::warn!(error = %error, "workspace projection sidecar persist failed");
        }
        if let Some(shared) = &self.shared_cache {
            if let Ok(mut slot) = shared.lock() {
                *slot = Some(cache);
            }
        }
    }
}

/// Pure capability gate for the optional model capability.
#[must_use]
#[expect(
    clippy::needless_pass_by_value,
    reason = "WO-2a requires the pure capability helper to take Option<String>"
)]
pub fn grants_model_capability(value: Option<String>) -> bool {
    value.as_deref().is_some_and(|key| !key.trim().is_empty())
}

fn load_or_default_config(root: &Path, workspace_id: &str) -> Result<WorkspaceConfig, TexoError> {
    let config_path = root.join(".texo").join("config.toml");
    if config_path.exists() {
        return TexoRootConfig::load(&config_path)
            .map_err(config_error)?
            .resolve(Some(workspace_id))
            .map_err(config_error);
    }
    Ok(workspace_config_from_entry(
        workspace_id,
        WorkspaceEntry::for_id(workspace_id),
    ))
}

fn load_or_init_config(root: &Path, workspace_id: &str) -> Result<WorkspaceConfig, TexoError> {
    let config_path = root.join(".texo").join("config.toml");
    if config_path.exists() {
        let root_config = TexoRootConfig::load(&config_path).map_err(config_error)?;
        return match root_config.resolve(Some(workspace_id)) {
            Ok(config) => Ok(config),
            Err(ConfigError::UnknownWorkspace(_)) => Ok(workspace_config_from_entry(
                workspace_id,
                WorkspaceEntry::for_id(workspace_id),
            )),
            Err(error) => Err(config_error(error)),
        };
    }
    let entry = WorkspaceEntry::for_id(workspace_id);
    Ok(workspace_config_from_entry(workspace_id, entry))
}

fn workspace_config_from_entry(workspace_id: &str, entry: WorkspaceEntry) -> WorkspaceConfig {
    WorkspaceConfig {
        workspace_id: workspace_id.to_string(),
        store_path: entry.store_path,
        docs_glob: entry.docs_glob,
        extractor_cmd: entry.extractor_cmd,
        semantics: entry.semantics,
        gateway: None,
    }
}

fn open_store_for_config(
    root: &Path,
    config: &WorkspaceConfig,
) -> Result<Arc<Store<Open>>, TexoError> {
    batpak::event::validate_event_payload_registry().map_err(|error| TexoError::Registry {
        detail: error.to_string(),
    })?;
    let dir = config.store_path_buf(root);
    Ok(Arc::new(Store::open(StoreConfig::new(dir))?))
}

fn op_receipt_coordinate(workspace_id: &str) -> Result<Coordinate, TexoError> {
    Coordinate::new(
        format!("op-receipts:{workspace_id}"),
        format!("ops:{workspace_id}"),
    )
    .map_err(TexoError::from)
}

fn compute_fingerprints() -> Result<HostFingerprints, TexoError> {
    let interface = fingerprint::canonical_interface(&catalog());
    let module_digest = digest_hex("texo.module.v2", &interface)?;
    let host_fingerprint = digest_hex("texo.host.v2", &interface)?;
    let interface_fingerprint = interface.interface_fingerprint;
    Ok(HostFingerprints {
        module_digest,
        host_fingerprint,
        interface_fingerprint,
    })
}

fn digest_hex<T: Serialize>(domain: &str, value: &T) -> Result<String, TexoError> {
    let mut bytes = domain.as_bytes().to_vec();
    let encoded = batpak::canonical::to_bytes(value).map_err(|error| TexoError::Host {
        detail: format!("fingerprint canonical encoding failed: {error}"),
    })?;
    bytes.extend_from_slice(&encoded);
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn config_error(error: crate::config::ConfigError) -> TexoError {
    TexoError::Config {
        detail: error.to_string(),
        source: Some(Box::new(error)),
    }
}

fn build_error(error: impl std::fmt::Display) -> TexoError {
    TexoError::Host {
        detail: error.to_string(),
    }
}

fn runtime_error(error: RuntimeError) -> TexoError {
    match error {
        RuntimeError::Denied {
            name,
            code,
            message,
        } => TexoError::OpRuntime {
            op: name,
            detail: format!("{code}: {message}"),
            denied: true,
        },
        RuntimeError::Handler {
            name,
            code,
            message,
        } => TexoError::OpRuntime {
            op: name,
            detail: format!("{code}: {message}"),
            denied: false,
        },
        RuntimeError::UnknownOperation { name } => TexoError::OpRuntime {
            op: name,
            detail: "unknown operation".to_string(),
            denied: false,
        },
        RuntimeError::MissingHandler { name } => TexoError::OpRuntime {
            op: name,
            detail: "missing handler".to_string(),
            denied: false,
        },
        RuntimeError::ReceiptSink { name, message, .. }
        | RuntimeError::StatusSink { name, message, .. } => TexoError::OpRuntime {
            op: name,
            detail: message,
            denied: false,
        },
        _ => TexoError::OpRuntime {
            op: "unknown".to_string(),
            detail: error.to_string(),
            denied: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_capability_gate_is_non_empty_key_only() {
        assert!(!grants_model_capability(None));
        assert!(!grants_model_capability(Some(String::new())));
        assert!(!grants_model_capability(Some("  ".to_string())));
        assert!(grants_model_capability(Some("sk-test".to_string())));
    }
}
