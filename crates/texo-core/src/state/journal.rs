//! Journal typestate markers.

use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use crate::config::TexoConfig;
use crate::error::TexoError;
use crate::journal::store::StoreHandle;
use crate::replay::reducer::ReplayedState;
use crate::types::ids::WorkspaceId;

/// Marker: journal is open for reads and writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Open;

/// Marker: journal is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

/// Sealed journal state trait.
pub trait JournalState {}

impl JournalState for Open {}
impl JournalState for Closed {}

/// Typestate wrapper around the BatPak store handle.
pub struct Journal<State: JournalState> {
    pub(crate) handle: Option<StoreHandle>,
    pub(crate) config: TexoConfig,
    pub(crate) root: PathBuf,
    _state: PhantomData<State>,
}

impl Journal<Open> {
    /// Open a journal at the configured store path.
    pub fn open(config: TexoConfig, root: &Path) -> Result<Self, TexoError> {
        let store_path = config.store_path_buf(root);
        if let Some(parent) = store_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let handle = StoreHandle::open(&store_path)?;
        Ok(Self {
            handle: Some(handle),
            config,
            root: root.to_path_buf(),
            _state: PhantomData,
        })
    }

    /// Borrow the underlying store handle.
    pub fn handle(&self) -> &StoreHandle {
        self.handle
            .as_ref()
            .expect("open journal must hold a store handle")
    }

    /// Borrow configuration.
    pub fn config(&self) -> &TexoConfig {
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
        if let Some(handle) = self.handle {
            handle.close()?;
        }
        Ok(Journal {
            handle: None,
            config: self.config,
            root: self.root,
            _state: PhantomData,
        })
    }
}

impl Journal<Closed> {
    /// Reopen a previously closed journal.
    pub fn reopen(self) -> Result<Journal<Open>, TexoError> {
        Journal::<Open>::open(self.config, &self.root)
    }
}
