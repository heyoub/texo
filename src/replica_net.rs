//! Bounded `syncbat`/`netbat` exposure of canonical replica source pages.

use std::collections::BTreeSet;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use batpak::store::{Open, Store};
use batpak::EventPayload;
use serde::{Deserialize, Serialize};
use syncbat::{Core, EffectClass, Handler, HandlerError, HandlerResult, OperationDescriptor};

use crate::compat::batpak::{self as substrate, RemoteImportEvent};
use crate::error::{SurfaceKind, TexoError};
use crate::events::payloads::ReplicaBatchMaterializedV1;
use crate::surfaces::http::server::ShutdownHandle;

/// Stable operation name exported through `netbat`.
pub const PAGE_OPERATION: &str = "texo.replica.page.read";
/// Maximum decoded page response. The wire is hex and therefore twice this.
pub const MAX_RESPONSE_BYTES: usize = 20 * 1024 * 1024;
/// Absolute per-call read/write deadline.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

const PAGE_DESCRIPTOR: OperationDescriptor = OperationDescriptor::new(
    PAGE_OPERATION,
    EffectClass::Inspect,
    "texo.replica.page.input.v2",
    "texo.replica.page.output.v1",
    "texo.replica.page.receipt.v1",
);

/// Authenticated request for one bounded source page.
#[derive(Serialize, Deserialize)]
pub struct PageRequest {
    /// Wire schema.
    pub schema_version: u32,
    /// Keyed authentication tag over every remaining request field.
    pub auth_tag: [u8; 32],
    /// Expected workspace identity.
    pub workspace_id: String,
    /// Expected canonical journal identity.
    pub source_journal: String,
    /// Exclusive source sequence cursor.
    pub after: Option<u64>,
    /// Anchor event expected at `after`.
    pub after_anchor_event_id_hex: Option<String>,
    /// Frozen source ceiling, selected by the first response.
    pub source_ceiling: Option<u64>,
}

#[derive(Serialize)]
struct PageAuthBody<'request> {
    schema_version: u32,
    workspace_id: &'request str,
    source_journal: &'request str,
    after: Option<u64>,
    after_anchor_event_id_hex: &'request Option<String>,
    source_ceiling: Option<u64>,
}

impl PageRequest {
    /// Build a request authenticated by a secret that never appears on the wire.
    ///
    /// # Errors
    /// Returns a canonical encoding error for the authenticated request body.
    pub fn authenticated(
        secret: &str,
        workspace_id: String,
        source_journal: String,
        after: Option<u64>,
        after_anchor_event_id_hex: Option<String>,
        source_ceiling: Option<u64>,
    ) -> Result<Self, String> {
        let mut request = Self {
            schema_version: 2,
            auth_tag: [0_u8; 32],
            workspace_id,
            source_journal,
            after,
            after_anchor_event_id_hex,
            source_ceiling,
        };
        request.auth_tag = request.authentication_tag(secret)?;
        Ok(request)
    }

    fn authentication_tag(&self, secret: &str) -> Result<[u8; 32], String> {
        let body = PageAuthBody {
            schema_version: self.schema_version,
            workspace_id: &self.workspace_id,
            source_journal: &self.source_journal,
            after: self.after,
            after_anchor_event_id_hex: &self.after_anchor_event_id_hex,
            source_ceiling: self.source_ceiling,
        };
        let bytes = batpak::canonical::to_bytes(&body).map_err(|error| error.to_string())?;
        let key = blake3::derive_key("texo.replica.netbat.auth-key.v2", secret.as_bytes());
        let mut hasher = blake3::Hasher::new_keyed(&key);
        hasher.update(&bytes);
        Ok(*hasher.finalize().as_bytes())
    }
}

/// One bounded, source-anchored remote page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageResponse {
    /// Wire schema.
    pub schema_version: u32,
    /// Bound workspace identity.
    pub workspace_id: String,
    /// Bound canonical journal identity.
    pub source_journal: String,
    /// Source ceiling frozen for this traversal.
    pub source_ceiling: u64,
    /// Event id at the source ceiling, when nonzero.
    pub source_ceiling_anchor_event_id_hex: Option<String>,
    /// Highest source sequence represented by this response.
    pub high_watermark: Option<u64>,
    /// Event id at the high watermark, when present.
    pub high_watermark_anchor_event_id_hex: Option<String>,
    /// Source rows skipped because they are substrate-reserved.
    pub skipped_reserved: u64,
    /// Replica-ledger rows omitted from replica-of-replica traversal.
    pub skipped_operational: u64,
    /// Whether another bounded page remains below the ceiling.
    pub has_more: bool,
    /// Exact source events to materialize.
    pub events: Vec<RemoteImportEvent>,
}

/// Runtime parameters for the canonical replica-source listener.
pub struct Server {
    /// Already-bound listener, making port ownership testable before startup.
    pub listener: TcpListener,
    /// Canonical store shared with the main Texo server.
    pub store: Arc<Store<Open>>,
    /// Workspace served by this process.
    pub workspace_id: String,
    /// Canonical journal id served by this process.
    pub journal_id: String,
    /// Secret bytes loaded from the configured environment variable.
    pub token: String,
}

/// Build the shared bounded transport limits.
#[must_use]
pub fn limits() -> netbat::Limits {
    netbat::Limits::new()
        .with_max_input_bytes(64 * 1024)
        .with_max_output_bytes(MAX_RESPONSE_BYTES)
        .with_max_line_bytes(MAX_RESPONSE_BYTES * 2 + 4096)
        .with_max_stream_error_message_bytes(4096)
}

/// Serve canonical source pages until the shared Texo shutdown is requested.
///
/// # Errors
/// Returns listener, timeout, core-composition, or fatal transport failures.
pub fn serve(
    server: &Server,
    shutdown: &ShutdownHandle,
) -> Result<netbat::TcpServeStats, TexoError> {
    server.listener.set_nonblocking(true).map_err(surface)?;
    let mut stats = netbat::TcpServeStats::default();
    while !shutdown.is_shutdown() {
        match server.listener.accept() {
            Ok((stream, _address)) => {
                stats.accepted_connections = stats.accepted_connections.saturating_add(1);
                match serve_connection(stream, server)? {
                    Ok(true) => stats.served_requests = stats.served_requests.saturating_add(1),
                    Ok(false) => stats.failed_requests = stats.failed_requests.saturating_add(1),
                    Err(error) if connection_error(&error) => {
                        stats.connection_io_failures =
                            stats.connection_io_failures.saturating_add(1);
                    }
                    Err(error) => return Err(surface(error)),
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(surface(error)),
        }
    }
    stats.shutdown_requested = true;
    Ok(stats)
}

fn serve_connection(
    stream: TcpStream,
    server: &Server,
) -> Result<Result<bool, netbat::NetbatError>, TexoError> {
    stream
        .set_read_timeout(Some(REQUEST_TIMEOUT))
        .map_err(surface)?;
    stream
        .set_write_timeout(Some(REQUEST_TIMEOUT))
        .map_err(surface)?;
    let mut core = build_core(server)?;
    let mut stream = stream;
    Ok(netbat::serve_stream(&mut stream, &mut core, &limits())
        .map(|_| true)
        .or_else(|error| {
            if connection_error(&error) {
                Err(error)
            } else {
                Ok(false)
            }
        }))
}

fn build_core(server: &Server) -> Result<Core, TexoError> {
    let handler = PageHandler {
        store: Arc::clone(&server.store),
        workspace_id: server.workspace_id.clone(),
        journal_id: server.journal_id.clone(),
        token: server.token.clone(),
    };
    let mut builder = Core::builder();
    builder
        .register(PAGE_DESCRIPTOR, handler)
        .map_err(|error| TexoError::Host {
            detail: error.to_string(),
        })?;
    builder.without_receipts();
    builder.build().map_err(|error| TexoError::Host {
        detail: error.to_string(),
    })
}

struct PageHandler {
    store: Arc<Store<Open>>,
    workspace_id: String,
    journal_id: String,
    token: String,
}

impl Handler for PageHandler {
    fn handle(&mut self, input: &[u8], _context: &mut syncbat::Ctx<'_>) -> HandlerResult {
        let request: PageRequest = batpak::canonical::from_bytes(input)
            .map_err(|error| HandlerError::invalid_input(error.to_string()))?;
        if request.schema_version != 2 {
            return Err(HandlerError::invalid_input(
                "unsupported replica request schema",
            ));
        }
        let expected_tag = request
            .authentication_tag(&self.token)
            .map_err(|error| HandlerError::failed(error.clone()))?;
        if !constant_time_equal(&request.auth_tag, &expected_tag) {
            return Err(HandlerError::failed("replica authentication failed"));
        }
        if request.workspace_id != self.workspace_id || request.source_journal != self.journal_id {
            return Err(HandlerError::invalid_input(
                "replica source binding mismatch",
            ));
        }
        validate_request_anchor(&self.store, &request)?;
        let current = self.store.frontier().visible_hlc.global_sequence;
        let source_ceiling = request.source_ceiling.unwrap_or(current);
        if source_ceiling > current {
            return Err(HandlerError::invalid_input(
                "source ceiling exceeds visible frontier",
            ));
        }
        let page = substrate::read_import_page(
            &self.store,
            request.after,
            source_ceiling,
            &BTreeSet::new(),
            ReplicaBatchMaterializedV1::KIND,
        )
        .map_err(|error| HandlerError::failed(error.to_string()))?;
        let response = PageResponse {
            schema_version: 1,
            workspace_id: self.workspace_id.clone(),
            source_journal: self.journal_id.clone(),
            source_ceiling,
            source_ceiling_anchor_event_id_hex: substrate::event_id_at(&self.store, source_ceiling),
            high_watermark: page.high_watermark,
            high_watermark_anchor_event_id_hex: page
                .high_watermark
                .and_then(|sequence| substrate::event_id_at(&self.store, sequence)),
            skipped_reserved: page.skipped_reserved,
            skipped_operational: page.skipped_operational,
            has_more: page.has_more,
            events: page
                .events
                .into_iter()
                .map(substrate::ImportEvent::into_remote)
                .collect(),
        };
        batpak::canonical::to_bytes(&response)
            .map_err(|error| HandlerError::failed(error.to_string()))
    }
}

fn validate_request_anchor(store: &Store<Open>, request: &PageRequest) -> Result<(), HandlerError> {
    match (request.after, &request.after_anchor_event_id_hex) {
        (None, None) => Ok(()),
        (Some(sequence), Some(expected))
            if substrate::event_id_at(store, sequence).as_ref() == Some(expected) =>
        {
            Ok(())
        }
        _ => Err(HandlerError::invalid_input("source cursor anchor mismatch")),
    }
}

fn constant_time_equal(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn connection_error(error: &netbat::NetbatError) -> bool {
    matches!(
        error,
        netbat::NetbatError::Io { .. } | netbat::NetbatError::EmptyStream
    )
}

fn surface(error: impl std::fmt::Display) -> TexoError {
    TexoError::Surface {
        which: SurfaceKind::Http,
        detail: format!("replica netbat boundary: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_comparison_is_exact() {
        let request = PageRequest::authenticated(
            "same",
            "workspace".to_string(),
            "journal".to_string(),
            None,
            None,
            None,
        )
        .expect("request");
        assert!(constant_time_equal(
            &request.auth_tag,
            &request.authentication_tag("same").expect("same tag")
        ));
        assert!(!constant_time_equal(
            &request.auth_tag,
            &request
                .authentication_tag("different")
                .expect("different tag")
        ));
        let wire = batpak::canonical::to_bytes(&request).expect("wire");
        assert!(!wire.windows("same".len()).any(|bytes| bytes == b"same"));
    }
}
