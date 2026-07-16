//! Thread-local operation environment.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use crate::error::TexoError;

mod context;

pub use context::{EnvGuard, OpEnv};

/// Compact receipt note returned by operation JSON outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptNote {
    /// Appended event id as lowercase hex.
    pub event_id_hex: String,
    /// `BatPak` event kind raw bits.
    pub kind_bits: u16,
    /// Global sequence assigned by the store.
    pub global_sequence: u64,
}

thread_local! {
    static ENV: RefCell<Option<Rc<OpEnv>>> = const { RefCell::new(None) };
    static REPLAY_DEPTH: Cell<u32> = const { Cell::new(0) };
}

struct ReplayGuard;

impl Drop for ReplayGuard {
    fn drop(&mut self) {
        REPLAY_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Execute the common deterministic projection boundary while model transport
/// is denied. All workspace assembly and direct `BatPak` projection entrypoints
/// must pass through this function.
pub(crate) fn deterministic_projection<T>(f: impl FnOnce() -> T) -> T {
    REPLAY_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
    let guard = ReplayGuard;
    let output = f();
    drop(guard);
    output
}

/// Compatibility name for deterministic projection replay.
pub(crate) fn replay_scope<T>(f: impl FnOnce() -> T) -> T {
    deterministic_projection(f)
}

/// True only when the current thread is outside deterministic replay.
pub(crate) fn model_calls_allowed() -> bool {
    REPLAY_DEPTH.with(|depth| depth.get() == 0)
}

/// Install an operation environment for the current thread.
#[must_use]
pub fn install(env: Rc<OpEnv>) -> EnvGuard {
    let previous = ENV.with(|slot| slot.replace(Some(env)));
    EnvGuard { previous }
}

/// Run a closure with the current operation environment.
///
/// # Errors
/// Returns [`TexoError::OpRuntime`] when no environment is installed.
pub fn with<T>(f: impl FnOnce(&OpEnv) -> T) -> Result<T, TexoError> {
    ENV.with(|slot| {
        let borrowed = slot.borrow();
        let Some(env) = borrowed.as_ref() else {
            return Err(TexoError::OpRuntime {
                op: "op.env".to_string(),
                detail: "no op environment installed".to_string(),
                denied: false,
            });
        };
        Ok(f(env))
    })
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        ENV.with(|slot| {
            let _old = slot.replace(self.previous.take());
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use batpak::store::{Store, StoreConfig};
    use tempfile::TempDir;

    use super::*;
    use crate::claims::workspace::WorkspaceCache;
    use crate::config::WorkspaceConfig;
    use crate::host::HostInterface;
    use crate::journal_store::JournalStore;
    use crate::topology::{JournalId, JournalRole, ResolvedJournal};

    fn test_env(root: &TempDir, workspace_id: &str) -> Rc<OpEnv> {
        let store =
            Store::open(StoreConfig::new(root.path().join("store"))).expect("test store opens");
        Rc::new(OpEnv {
            store: JournalStore::writable(Arc::new(store)),
            workspace_id: workspace_id.to_string(),
            root: root.path().to_path_buf(),
            config: WorkspaceConfig::demo(),
            cache: RefCell::new(WorkspaceCache::default()),
            receipts: RefCell::new(Vec::new()),
            observed_at_ms: 1,
            host_interface: test_host_interface(),
            journal: ResolvedJournal {
                id: JournalId::new("canonical").expect("valid test journal id"),
                role: JournalRole::Canonical,
                store_path: "store".to_string(),
                source_journal: None,
                replica_mode: None,
                source_endpoint: None,
                source_token_env: None,
            },
        })
    }

    fn test_host_interface() -> HostInterface {
        HostInterface {
            schema: "hostbat.interface.v1".to_string(),
            version: "test".to_string(),
            fingerprints: crate::host::HostFingerprints {
                module_digest: "00".repeat(32),
                host_fingerprint: "00".repeat(32),
                interface_fingerprint: "00".repeat(32),
            },
            operations: Vec::new(),
        }
    }

    #[test]
    fn with_errors_when_empty() {
        let error = with(|_| ()).expect_err("empty environment errors");
        assert_eq!(error.code(), "op.runtime");
    }

    #[test]
    fn nesting_restores_previous_environment() {
        let first_dir = TempDir::new().expect("first tempdir");
        let second_dir = TempDir::new().expect("second tempdir");
        let first = test_env(&first_dir, "first");
        let second = test_env(&second_dir, "second");

        let first_guard = install(Rc::clone(&first));
        assert_eq!(
            with(|env| env.workspace_id.clone()).expect("first installed"),
            "first"
        );
        {
            let second_guard = install(second);
            assert_eq!(
                with(|env| env.workspace_id.clone()).expect("second installed"),
                "second"
            );
            drop(second_guard);
        }
        assert_eq!(
            with(|env| env.workspace_id.clone()).expect("first restored"),
            "first"
        );
        drop(first_guard);

        let error = with(|_| ()).expect_err("environment removed");
        assert_eq!(error.code(), "op.runtime");
    }

    #[test]
    fn replay_guard_is_nested_and_restored() {
        assert!(model_calls_allowed());
        deterministic_projection(|| {
            assert!(!model_calls_allowed());
            deterministic_projection(|| assert!(!model_calls_allowed()));
            assert!(!model_calls_allowed());
        });
        assert!(model_calls_allowed());
    }
}
