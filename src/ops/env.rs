//! Thread-local operation environment.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use batpak::store::{Open, Store};
use serde::{Deserialize, Serialize};

use crate::claims::workspace::WorkspaceCache;
use crate::config::WorkspaceConfig;
use crate::error::TexoError;

/// Per-invocation environment installed around syncbat handler execution.
pub struct OpEnv {
    /// Open workspace store.
    pub store: Arc<Store<Open>>,
    /// Workspace identifier.
    pub workspace_id: String,
    /// Workspace root.
    pub root: PathBuf,
    /// Resolved workspace config.
    pub config: WorkspaceConfig,
    /// Per-host projection cache.
    pub cache: RefCell<WorkspaceCache>,
    /// Append receipts observed by the effect backend.
    pub receipts: RefCell<Vec<ReceiptNote>>,
    /// Deterministic operation timestamp supplied by surfaces.
    pub observed_at_ms: u64,
}

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
}

/// Guard that restores the previous operation environment on drop.
pub struct EnvGuard {
    previous: Option<Rc<OpEnv>>,
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

    fn test_env(root: &TempDir, workspace_id: &str) -> Rc<OpEnv> {
        let store =
            Store::open(StoreConfig::new(root.path().join("store"))).expect("test store opens");
        Rc::new(OpEnv {
            store: Arc::new(store),
            workspace_id: workspace_id.to_string(),
            root: root.path().to_path_buf(),
            config: WorkspaceConfig::demo(),
            cache: RefCell::new(WorkspaceCache::default()),
            receipts: RefCell::new(Vec::new()),
            observed_at_ms: 1,
        })
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
}
