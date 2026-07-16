use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::claims::workspace::WorkspaceCache;
use crate::config::WorkspaceConfig;
use crate::host::HostInterface;
use crate::journal_store::JournalStore;
use crate::topology::ResolvedJournal;

use super::ReceiptNote;

/// Per-invocation environment installed around syncbat handler execution.
pub struct OpEnv {
    /// Open workspace store.
    pub store: JournalStore,
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
    /// Actual mounted `hostbat` interface for the fingerprint operation.
    pub host_interface: HostInterface,
    /// Selected physical journal and its authority role.
    pub journal: ResolvedJournal,
}

/// Guard that restores the previous operation environment on drop.
pub struct EnvGuard {
    pub(super) previous: Option<Rc<OpEnv>>,
}
