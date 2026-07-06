//! Server-sent events over BatPak store notifications.

use std::io::Write;
use std::time::Duration;

use batpak::coordinate::Region;
use batpak::store::{Open, Store};
use serde_json::json;

use crate::error::{SurfaceKind, TexoError};
use crate::events::coordinate::scope_for_workspace;

use super::routes::{open_host, RouteState};

/// Serve one SSE connection.
///
/// # Errors
///
/// Returns [`TexoError::Surface`] when writes fail and other [`TexoError`]
/// variants when host/store setup fails.
pub fn serve(
    stream: &mut impl Write,
    state: &RouteState,
    keep_alive: Duration,
) -> Result<(), TexoError> {
    let host = open_host(state)?;
    let store = host.store();
    let scope = scope_for_workspace(host.workspace_id());
    let region = Region::scope(&scope);
    let frontier = store
        .by_scope(&scope)
        .into_iter()
        .map(|entry| entry.global_sequence())
        .max()
        .unwrap_or(0);
    let mut last_sequence = frontier;
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nCache-Control: no-cache, no-transform\r\n\r\n",
    ).map_err(surface_error)?;
    write_signal(stream, None, &json!({"kind":"hello","frontier":frontier}))?;

    // BatPak 0.9.0 APIs used here: Store::subscribe_lossy(&Region) creates a
    // Subscription, and Subscription::filtered_receiver() exposes the
    // writer-side region-filtered flume receiver for recv_timeout-driven SSE.
    let subscription = store.subscribe_lossy(&region);
    let rx = subscription.filtered_receiver();
    loop {
        match rx.recv_timeout(keep_alive) {
            Ok(notification) => {
                last_sequence = last_sequence.max(notification.sequence);
                write_signal(
                    stream,
                    Some(notification.sequence),
                    &json!({
                        "kind": "journal",
                        "sequence": notification.sequence,
                        "kind_bits": notification.kind.as_raw_u16()
                    }),
                )?;
            }
            Err(flume::RecvTimeoutError::Timeout) => {
                if !write_visible_since(stream, &store, &scope, &mut last_sequence)? {
                    stream
                        .write_all(b": keep-alive\n\n")
                        .map_err(surface_error)?;
                    stream.flush().map_err(surface_error)?;
                }
            }
            Err(flume::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn write_visible_since(
    stream: &mut impl Write,
    store: &Store<Open>,
    scope: &str,
    last_sequence: &mut u64,
) -> Result<bool, TexoError> {
    let mut entries = store
        .by_scope(scope)
        .into_iter()
        .filter(|entry| entry.global_sequence() > *last_sequence)
        .collect::<Vec<_>>();
    entries.sort_by_key(batpak::store::IndexEntry::global_sequence);
    let emitted = !entries.is_empty();
    for entry in entries {
        *last_sequence = entry.global_sequence();
        write_signal(
            stream,
            Some(entry.global_sequence()),
            &json!({
                "kind": "journal",
                "sequence": entry.global_sequence(),
                "kind_bits": entry.event_kind().as_raw_u16()
            }),
        )?;
    }
    Ok(emitted)
}

fn write_signal(
    stream: &mut impl Write,
    id: Option<u64>,
    data: &serde_json::Value,
) -> Result<(), TexoError> {
    if let Some(id) = id {
        writeln!(stream, "id: {id}").map_err(surface_error)?;
    }
    let frame = json!({"type":"signal","data":data});
    writeln!(stream, "data: {frame}\n").map_err(surface_error)?;
    stream.flush().map_err(surface_error)
}

fn surface_error(error: impl std::fmt::Display) -> TexoError {
    TexoError::Surface {
        which: SurfaceKind::Http,
        detail: error.to_string(),
    }
}
