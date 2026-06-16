//! Journal typestate markers.
//!
//! The journal's open/closed status is encoded structurally: each state type
//! OWNS the data that state requires. `Open(StoreHandle)` carries a
//! non-optional store handle, so an open-without-handle state is simply not
//! representable and no runtime check is needed to borrow it. `Closed` carries
//! no handle.

use std::path::{Path, PathBuf};

use crate::config::WorkspaceConfig;
use crate::error::TexoError;
use crate::journal::store::StoreHandle;
use crate::replay::reducer::ReplayedState;
use crate::types::ids::WorkspaceId;

/// State: journal is open for reads and writes, holding its store handle.
pub struct Open(StoreHandle);

/// State: journal is closed; no store handle is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

/// Sealed journal state trait.
pub trait JournalState {}

impl JournalState for Open {}
impl JournalState for Closed {}

/// Typestate wrapper around the BatPak store handle.
///
/// The `state` field carries the state-specific data: `Open` owns the
/// [`StoreHandle`], `Closed` owns nothing. This makes the open invariant
/// structural rather than checked at runtime.
pub struct Journal<State: JournalState> {
    pub(crate) state: State,
    pub(crate) config: WorkspaceConfig,
    pub(crate) root: PathBuf,
}

impl Journal<Open> {
    /// Open a journal at the configured store path.
    pub fn open(config: WorkspaceConfig, root: &Path) -> Result<Self, TexoError> {
        let store_path = config.store_path_buf(root);
        if let Some(parent) = store_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let handle = StoreHandle::open(&store_path)?;
        Ok(Self {
            state: Open(handle),
            config,
            root: root.to_path_buf(),
        })
    }

    /// Borrow the underlying store handle.
    ///
    /// Infallible: `Journal<Open>` structurally owns a [`StoreHandle`].
    pub fn handle(&self) -> &StoreHandle {
        &self.state.0
    }

    /// Borrow configuration.
    pub fn config(&self) -> &WorkspaceConfig {
        &self.config
    }

    /// Workspace root used to resolve relative paths.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Replay workspace state from the journal.
    pub fn replay(&self, workspace: &WorkspaceId) -> Result<ReplayedState, TexoError> {
        Ok(self.handle().replay_workspace(workspace, &self.config)?)
    }

    /// Close the journal cleanly.
    pub fn close(self) -> Result<Journal<Closed>, TexoError> {
        self.state.0.close()?;
        Ok(Journal {
            state: Closed,
            config: self.config,
            root: self.root,
        })
    }
}

impl Journal<Closed> {
    /// Reopen a previously closed journal.
    pub fn reopen(self) -> Result<Journal<Open>, TexoError> {
        Journal::<Open>::open(self.config, &self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init_workspace;

    /// Drive the full typestate cycle on a real on-disk store: open carries the
    /// handle, `root()` returns the workspace root, `close()` consumes `Open`
    /// and yields `Closed`, and `reopen()` consumes `Closed` and yields a fresh
    /// `Open` rooted at the same path. This exercises the open/close/handle/root
    /// transitions structurally rather than through the lib wrappers.
    #[test]
    fn open_use_close_reopen_cycle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let config = init_workspace(root, "demo").expect("init workspace");

        let open = Journal::<Open>::open(config, root).expect("open journal");
        // `root()` returns the workspace root the journal was opened against.
        assert_eq!(open.root(), root, "root() must report the open root");
        // `handle()` is infallible on an open journal and borrows the live store.
        let _handle = open.handle();
        let workspace = open.config().workspace().expect("workspace id");
        // A clean replay of the empty workspace must succeed through the handle.
        let replayed = open.replay(&workspace).expect("replay empty workspace");
        // An empty journal replays to a zero frontier.
        assert_eq!(replayed.state.replayed_through_sequence, 0);

        // close() transitions Open -> Closed.
        let closed: Journal<Closed> = open.close().expect("close journal");
        assert_eq!(closed.state, Closed, "closed state carries no handle");

        // reopen() transitions Closed -> Open at the same root.
        let reopened = closed.reopen().expect("reopen journal");
        // reopen() preserves the workspace root.
        assert_eq!(reopened.root(), root);
        reopened.close().expect("close reopened journal");
    }
}
