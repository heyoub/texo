//! Host identity and runtime shapes re-exported by the host facade.

use std::rc::Rc;

use serde::{Deserialize, Serialize};

use super::SharedWorkspaceCache;
use crate::ops::env::OpEnv;

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

/// One public operation in the mounted `hostbat` interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostOperationView {
    /// Stable operation name.
    pub name: String,
    /// Stable effect class spelling.
    pub effect: String,
    /// Stable receipt schema reference.
    pub receipt_kind: String,
}

/// Client-visible projection of the actual mounted `hostbat` composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInterface {
    /// Interface schema identifier.
    pub schema: String,
    /// Texo binary version.
    pub version: String,
    /// Content identities produced by `hostbat`.
    pub fingerprints: HostFingerprints,
    /// Canonically ordered mounted operations.
    pub operations: Vec<HostOperationView>,
}

/// Runnable Texo host over a content-identified `hostbat` composition.
pub struct TexoHost {
    pub(super) host: hostbat::Host,
    pub(super) env: Rc<OpEnv>,
    pub(super) interface: HostInterface,
    pub(super) shared_cache: Option<SharedWorkspaceCache>,
}
