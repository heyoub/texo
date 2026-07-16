//! Host composition for texo operations.

pub mod module;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use batpak::coordinate::Coordinate;
use batpak::store::{Open, ReadOnly, Store, StoreConfig};
use syncbat::{RuntimeError, StoreOperationStatusSink, StoreReceiptSink};

use crate::claims::workspace::WorkspaceCache;
use crate::config::{ConfigError, TexoRootConfig, WorkspaceConfig, WorkspaceEntry};
use crate::error::TexoError;
use crate::gateway::{resolve_role, ModelRole, RoleOverrides};
use crate::journal_store::JournalStore;
use crate::ops::backend::TexoEffectBackend;
use crate::ops::env::{self, OpEnv};
use crate::topology::{JournalRole, ResolvedJournal};

mod model;

pub use model::{HostFingerprints, HostInterface, HostOperationView, TexoHost};

/// Cross-request checkout slot for the warm workspace projection.
pub type SharedWorkspaceCache = Arc<Mutex<Option<WorkspaceCache>>>;

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
    let (config, _journal) = root_config
        .resolve_journal(Some(workspace_id), None)
        .map_err(config_error)?;
    open_store_for_config(root, &config)
}

/// Open one explicitly selected workspace journal with authority encoded in
/// the returned handle.
///
/// # Errors
/// Returns configuration, registry, or store-open failures.
pub fn open_workspace_journal_store(
    root: impl AsRef<Path>,
    workspace_id: &str,
    journal_id: &str,
) -> Result<JournalStore, TexoError> {
    let root = root.as_ref();
    let config_path = root.join(".texo").join("config.toml");
    let root_config = TexoRootConfig::load(&config_path).map_err(config_error)?;
    let (config, journal) = root_config
        .resolve_journal(Some(workspace_id), Some(journal_id))
        .map_err(config_error)?;
    open_journal_store_for_config(root, &config, journal.role)
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
        let (config, journal) = load_or_default_config(&root, &workspace_id, None)?;
        let store = open_journal_store_for_config(&root, &config, journal.role)?;
        Self::from_parts(
            root,
            &workspace_id,
            observed_at_ms,
            config,
            journal,
            store,
            None,
        )
    }

    /// Build a runnable host for one explicitly selected physical journal.
    ///
    /// # Errors
    /// Returns configuration, store, registry, or host-composition failures.
    pub fn open_journal(
        root: impl Into<PathBuf>,
        workspace_id: impl Into<String>,
        journal_id: &str,
        observed_at_ms: u64,
    ) -> Result<Self, TexoError> {
        let root = root.into();
        let workspace_id = workspace_id.into();
        let (config, journal) = load_or_default_config(&root, &workspace_id, Some(journal_id))?;
        let store = open_journal_store_for_config(&root, &config, journal.role)?;
        Self::from_parts(
            root,
            &workspace_id,
            observed_at_ms,
            config,
            journal,
            store,
            None,
        )
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
        let (config, journal) = load_or_init_config(&root, &workspace_id)?;
        let store = open_journal_store_for_config(&root, &config, journal.role)?;
        Self::from_parts(
            root,
            &workspace_id,
            observed_at_ms,
            config,
            journal,
            store,
            None,
        )
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
        let (config, journal) = load_or_default_config(&root, &workspace_id, None)?;
        Self::from_parts(
            root,
            &workspace_id,
            observed_at_ms,
            config,
            journal,
            JournalStore::writable(store),
            None,
        )
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
        let (config, journal) = load_or_default_config(&root, &workspace_id, None)?;
        Self::from_parts(
            root,
            &workspace_id,
            observed_at_ms,
            config,
            journal,
            JournalStore::writable(store),
            Some(shared_cache),
        )
    }

    /// Build a host over an already-open explicitly selected journal.
    ///
    /// # Errors
    /// Returns registry, topology, or host-composition failures.
    pub fn open_journal_with_store_and_cache(
        root: impl Into<PathBuf>,
        workspace_id: impl Into<String>,
        journal_id: &str,
        observed_at_ms: u64,
        store: JournalStore,
        shared_cache: SharedWorkspaceCache,
    ) -> Result<Self, TexoError> {
        let root = root.into();
        let workspace_id = workspace_id.into();
        batpak::event::validate_event_payload_registry().map_err(|error| TexoError::Registry {
            detail: error.to_string(),
        })?;
        let (config, journal) = load_or_default_config(&root, &workspace_id, Some(journal_id))?;
        Self::from_parts(
            root,
            &workspace_id,
            observed_at_ms,
            config,
            journal,
            store,
            Some(shared_cache),
        )
    }

    fn from_parts(
        root: PathBuf,
        workspace_id: &str,
        observed_at_ms: u64,
        config: WorkspaceConfig,
        journal: ResolvedJournal,
        store: JournalStore,
        shared_cache: Option<SharedWorkspaceCache>,
    ) -> Result<Self, TexoError> {
        let module = module::build(journal.role)?;
        let module_digest = module.manifest().digest().to_hex();
        let mut builder = hostbat::HostBuilder::new()
            .mount(module)
            .map_err(build_error)?
            .effect_backend(TexoEffectBackend);
        if journal.role == JournalRole::Canonical {
            let writer = store.writable_arc().ok_or_else(|| TexoError::Host {
                detail: "canonical journal was not opened with write authority".to_string(),
            })?;
            builder = builder
                .receipt_sink(StoreReceiptSink::new(
                    Arc::clone(&writer),
                    op_receipt_coordinate(workspace_id)?,
                ))
                .status_sink(StoreOperationStatusSink::new(writer));
        }
        let model_role = resolve_role(
            ModelRole::Relate,
            &RoleOverrides::default(),
            config.gateway.as_ref(),
        );
        if grants_model_capability(Some(model_role.api_key.as_str())) {
            builder = builder.grant_capability("texo.cap.model");
        }
        let host = builder.build().map_err(build_error)?;
        let fingerprints = HostFingerprints {
            module_digest,
            host_fingerprint: host.fingerprint().to_hex(),
            interface_fingerprint: host.interface_fingerprint().to_hex(),
        };
        let interface = HostInterface {
            schema: "hostbat.interface.v1".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            fingerprints,
            operations: host
                .operations()
                .map(|descriptor| HostOperationView {
                    name: descriptor.name().to_string(),
                    effect: descriptor.effect.as_str().to_string(),
                    receipt_kind: descriptor.receipt_kind().to_string(),
                })
                .collect(),
        };
        let cache = shared_cache
            .as_ref()
            .and_then(|slot| slot.lock().ok()?.take())
            .unwrap_or_else(|| {
                WorkspaceCache::load_journal(&root, workspace_id, journal.id.as_str())
            });
        let env = Rc::new(OpEnv {
            store,
            workspace_id: workspace_id.to_string(),
            root,
            config,
            cache: std::cell::RefCell::new(cache),
            receipts: std::cell::RefCell::new(Vec::new()),
            observed_at_ms,
            host_interface: interface.clone(),
            journal,
        });
        Ok(Self {
            host,
            env,
            interface,
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
        let bytes = batpak::canonical::to_bytes(input).map_err(build_error)?;
        self.env.receipts.borrow_mut().clear();
        let _guard = env::install(Rc::clone(&self.env));
        let result = self.host.invoke(op, bytes).map_err(runtime_error)?;
        batpak::canonical::from_bytes(result.output()).map_err(build_error)
    }

    /// Return deterministic host fingerprints.
    #[must_use]
    pub fn fingerprints(&self) -> HostFingerprints {
        self.interface.fingerprints.clone()
    }

    /// Return the client-visible mounted interface.
    #[must_use]
    pub fn interface(&self) -> HostInterface {
        self.interface.clone()
    }

    /// Return the underlying store for tests and surfaces that need reads.
    #[must_use]
    pub fn store(&self) -> JournalStore {
        self.env.store.clone()
    }

    /// Return the workspace id this host runs against.
    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.env.workspace_id
    }

    /// Return the selected stable physical journal id.
    #[must_use]
    pub fn journal_id(&self) -> &crate::topology::JournalId {
        &self.env.journal.id
    }

    /// Return the selected journal authority role.
    #[must_use]
    pub fn journal_role(&self) -> JournalRole {
        self.env.journal.role
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
        // Move the cache out rather than cloning it: at scale the clone is
        // hundreds of megabytes of peak RSS per teardown.
        let mut cache = self.env.cache.take();
        if cache.is_dirty() {
            match cache.save_journal(
                &self.env.root,
                &self.env.workspace_id,
                self.env.journal.id.as_str(),
            ) {
                Ok(()) => cache.mark_clean(),
                Err(error) => {
                    tracing::warn!(error = %error, "workspace projection sidecar persist failed");
                }
            }
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
pub fn grants_model_capability(value: Option<&str>) -> bool {
    value.is_some_and(|key| !key.trim().is_empty())
}

fn load_or_default_config(
    root: &Path,
    workspace_id: &str,
    journal_id: Option<&str>,
) -> Result<(WorkspaceConfig, ResolvedJournal), TexoError> {
    let config_path = root.join(".texo").join("config.toml");
    if config_path.exists() {
        return TexoRootConfig::load(&config_path)
            .map_err(config_error)?
            .resolve_journal(Some(workspace_id), journal_id)
            .map_err(config_error);
    }
    workspace_config_from_entry(
        workspace_id,
        WorkspaceEntry::for_id(workspace_id),
        journal_id,
    )
}

fn load_or_init_config(
    root: &Path,
    workspace_id: &str,
) -> Result<(WorkspaceConfig, ResolvedJournal), TexoError> {
    let config_path = root.join(".texo").join("config.toml");
    if config_path.exists() {
        let root_config = TexoRootConfig::load(&config_path).map_err(config_error)?;
        return match root_config.resolve_journal(Some(workspace_id), None) {
            Ok(config) => Ok(config),
            Err(ConfigError::UnknownWorkspace(_)) => workspace_config_from_entry(
                workspace_id,
                WorkspaceEntry::for_id(workspace_id),
                None,
            ),
            Err(error) => Err(config_error(error)),
        };
    }
    let entry = WorkspaceEntry::for_id(workspace_id);
    workspace_config_from_entry(workspace_id, entry, None)
}

fn workspace_config_from_entry(
    workspace_id: &str,
    entry: WorkspaceEntry,
    journal_id: Option<&str>,
) -> Result<(WorkspaceConfig, ResolvedJournal), TexoError> {
    let journal =
        crate::topology::resolve_journal(&entry.primary_journal, &entry.journals, journal_id)
            .map_err(ConfigError::from)
            .map_err(config_error)?;
    let config = WorkspaceConfig {
        workspace_id: workspace_id.to_string(),
        store_path: journal.store_path.clone(),
        docs_glob: entry.docs_glob,
        extractor_cmd: entry.extractor_cmd,
        semantics: entry.semantics,
        gateway: None,
    };
    Ok((config, journal))
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

fn open_journal_store_for_config(
    root: &Path,
    config: &WorkspaceConfig,
    role: JournalRole,
) -> Result<JournalStore, TexoError> {
    batpak::event::validate_event_payload_registry().map_err(|error| TexoError::Registry {
        detail: error.to_string(),
    })?;
    let store_config = StoreConfig::new(config.store_path_buf(root));
    match role {
        JournalRole::Canonical => Ok(JournalStore::writable(Arc::new(Store::open(store_config)?))),
        JournalRole::Replica => Ok(JournalStore::read_only(Arc::new(
            Store::<ReadOnly>::open_read_only(store_config)?,
        ))),
    }
}

fn op_receipt_coordinate(workspace_id: &str) -> Result<Coordinate, TexoError> {
    Coordinate::new(
        format!("op-receipts:{workspace_id}"),
        format!("ops:{workspace_id}"),
    )
    .map_err(TexoError::from)
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
        } => {
            if code == "invalid_input" {
                TexoError::OpInput {
                    detail: input_error_detail(&name, &message),
                    op: name,
                }
            } else if let Some((kind, detail)) = snapshot_error_detail(&message) {
                TexoError::Snapshot { kind, detail }
            } else {
                TexoError::OpRuntime {
                    op: name,
                    detail: format!("{code}: {message}"),
                    denied: false,
                }
            }
        }
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

fn input_error_detail(operation: &str, message: &str) -> String {
    let message = message.strip_prefix("op.input: ").unwrap_or(message);
    let operation_prefix = format!("op input {operation}: ");
    message
        .strip_prefix(&operation_prefix)
        .unwrap_or(message)
        .to_string()
}

fn snapshot_error_detail(message: &str) -> Option<(crate::error::SnapshotFailureKind, String)> {
    use crate::error::SnapshotFailureKind;
    let classes = [
        ("snapshot.invalid", SnapshotFailureKind::InvalidToken),
        ("snapshot.unavailable", SnapshotFailureKind::Unavailable),
        ("snapshot.anchor", SnapshotFailureKind::AnchorMismatch),
        (
            "snapshot.source_unavailable",
            SnapshotFailureKind::SourceUnavailable,
        ),
    ];
    classes.into_iter().find_map(|(code, kind)| {
        let offset = message.find(code)?;
        let detail = message[offset + code.len()..]
            .trim_start_matches([':', ' '])
            .to_string();
        Some((kind, detail))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_capability_gate_is_non_empty_key_only() {
        assert!(!grants_model_capability(None));
        assert!(!grants_model_capability(Some("")));
        assert!(!grants_model_capability(Some("  ")));
        assert!(grants_model_capability(Some("sk-test")));
    }

    #[test]
    fn runtime_error_preserves_invalid_input_class_and_detail() {
        let error = runtime_error(RuntimeError::handler(
            "texo.claims.search",
            "invalid_input",
            "op.input: op input texo.claims.search: limit must be between 1 and 100",
        ));
        assert!(matches!(
            error,
            TexoError::OpInput { ref op, ref detail }
                if op == "texo.claims.search" && detail == "limit must be between 1 and 100"
        ));
        assert_eq!(
            error.facts(),
            crate::error::FailureFacts {
                committed: crate::error::Committed::No,
                retry_safe: true,
                resume: Some("fix the input and retry"),
            }
        );
    }
}
