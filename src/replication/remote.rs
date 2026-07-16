use std::collections::BTreeSet;
use std::path::Path;

use batpak::store::{Open, Store};

use crate::compat::batpak as substrate;
use crate::error::{Committed, ReplicationFailureKind, TexoError};
use crate::replica_net::{self, PageRequest, PageResponse};

use super::evidence::{
    cursor_path, display_relative, frontier, open_store, persist, replication_error,
    source_namespace, unchanged_report,
};
use super::materialize::{replica_ledger_item, represented_source_events, verify_replica};
use super::{Circuit, ReplicaCursor, ReplicaReport, STATE_SCHEMA_VERSION};

#[derive(Default)]
struct RemoteProgress {
    after: Option<u64>,
    after_anchor: Option<String>,
    source_ceiling: Option<u64>,
    source_ceiling_anchor: Option<String>,
    imported: u64,
    deduplicated: u64,
    skipped_reserved: u64,
    skipped_operational: u64,
}

pub(super) fn import_remote_from_cursor(
    root: &Path,
    circuit: &Circuit,
    previous: Option<&ReplicaCursor>,
) -> Result<ReplicaReport, TexoError> {
    let (endpoint, token) = remote_credentials(circuit)?;
    let namespace = source_namespace(&circuit.workspace.workspace_id, circuit.source.id.as_str());
    let mut progress = RemoteProgress {
        after: previous.and_then(|cursor| cursor.source_high_watermark),
        after_anchor: previous.and_then(|cursor| cursor.source_anchor_event_id_hex.clone()),
        ..RemoteProgress::default()
    };
    let first = request_remote_page(circuit, endpoint, &token, &progress)?;
    validate_remote_response(
        circuit,
        progress.after,
        None,
        None,
        &first,
        progress.imported,
    )?;
    if let Some(cursor) = previous.filter(|_| {
        first.high_watermark == progress.after
            && !first.has_more
            && first.events.is_empty()
            && first.source_ceiling == progress.after.unwrap_or(0)
    }) {
        return Ok(unchanged_report(root, circuit, cursor));
    }
    let destination = open_store(&circuit.replica_path, Committed::No)?;
    let mut represented = represented_source_events(
        &destination,
        &circuit.workspace.workspace_id,
        circuit.replica.id.as_str(),
        &namespace,
    )?;
    let mut next_response = Some(first);

    loop {
        let response = if let Some(first) = next_response.take() {
            first
        } else {
            request_remote_page(circuit, endpoint, &token, &progress)?
        };
        let has_more = response.has_more;
        materialize_remote_page(
            circuit,
            &destination,
            &namespace,
            &mut represented,
            &mut progress,
            response,
        )?;
        if !has_more {
            break;
        }
    }

    finish_remote_import(root, circuit, endpoint, &destination, namespace, progress)
}

fn remote_credentials(circuit: &Circuit) -> Result<(&str, String), TexoError> {
    let endpoint = circuit.replica.source_endpoint.as_deref().ok_or_else(|| {
        replication_error(
            ReplicationFailureKind::InvalidTopology,
            Committed::No,
            "remote replica has no source endpoint",
        )
    })?;
    let token_env = circuit.replica.source_token_env.as_deref().ok_or_else(|| {
        replication_error(
            ReplicationFailureKind::InvalidTopology,
            Committed::No,
            "remote replica has no token environment variable",
        )
    })?;
    let token = std::env::var(token_env).map_err(|_| {
        replication_error(
            ReplicationFailureKind::InvalidTopology,
            Committed::No,
            format!("remote replica token environment variable `{token_env}` is not set"),
        )
    })?;
    if token.is_empty() {
        Err(replication_error(
            ReplicationFailureKind::InvalidTopology,
            Committed::No,
            format!("remote replica token environment variable `{token_env}` is empty"),
        ))
    } else {
        Ok((endpoint, token))
    }
}

fn request_remote_page(
    circuit: &Circuit,
    endpoint: &str,
    token: &str,
    progress: &RemoteProgress,
) -> Result<PageResponse, TexoError> {
    let request = PageRequest::authenticated(
        token,
        circuit.workspace.workspace_id.clone(),
        circuit.source.id.to_string(),
        progress.after,
        progress.after_anchor.clone(),
        progress.source_ceiling,
    )
    .map_err(|error| {
        replication_error(
            ReplicationFailureKind::Evidence,
            committed_after(progress.imported),
            format!("authenticate remote replica page request: {error}"),
        )
    })?;
    let input = batpak::canonical::to_bytes(&request).map_err(|error| {
        replication_error(
            ReplicationFailureKind::Evidence,
            committed_after(progress.imported),
            format!("encode remote replica page request: {error}"),
        )
    })?;
    let response = crate::compat::netbat::call(
        endpoint,
        replica_net::PAGE_OPERATION,
        &input,
        &replica_net::limits(),
        replica_net::REQUEST_TIMEOUT,
    )
    .map_err(|error| {
        replication_error(
            ReplicationFailureKind::Transport,
            committed_after(progress.imported),
            format!("remote replica page call: {error}"),
        )
    })?;
    batpak::canonical::from_bytes(&response.into_bytes()).map_err(|error| {
        replication_error(
            ReplicationFailureKind::Transport,
            committed_after(progress.imported),
            format!("decode remote replica page: {error}"),
        )
    })
}

fn materialize_remote_page(
    circuit: &Circuit,
    destination: &Store<Open>,
    namespace: &str,
    represented: &mut BTreeSet<String>,
    progress: &mut RemoteProgress,
    response: PageResponse,
) -> Result<(), TexoError> {
    validate_remote_response(
        circuit,
        progress.after,
        progress.source_ceiling,
        progress.source_ceiling_anchor.as_deref(),
        &response,
        progress.imported,
    )?;
    let next = response.high_watermark;
    if next == progress.after && response.has_more {
        return Err(replication_error(
            ReplicationFailureKind::Verification,
            committed_after(progress.imported),
            "remote replica page made no cursor progress",
        ));
    }
    let events = validate_remote_events(&response, progress, represented)?;
    if !events.is_empty() {
        let ledger = replica_ledger_item(circuit, namespace, &events)?;
        let count = substrate::append_with_ledger(destination, namespace, events, ledger).map_err(
            |error| {
                replication_error(
                    ReplicationFailureKind::Substrate,
                    committed_after(progress.imported),
                    format!("atomic remote replica batch failed: {error}"),
                )
            },
        )?;
        progress.imported = progress
            .imported
            .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }
    progress.source_ceiling = Some(response.source_ceiling);
    progress
        .source_ceiling_anchor
        .clone_from(&response.source_ceiling_anchor_event_id_hex);
    progress.skipped_reserved = progress
        .skipped_reserved
        .saturating_add(response.skipped_reserved);
    progress.skipped_operational = progress
        .skipped_operational
        .saturating_add(response.skipped_operational);
    progress.after = next;
    progress.after_anchor = response.high_watermark_anchor_event_id_hex;
    Ok(())
}

fn validate_remote_events(
    response: &PageResponse,
    progress: &mut RemoteProgress,
    represented: &mut BTreeSet<String>,
) -> Result<Vec<substrate::ImportEvent>, TexoError> {
    let mut events = Vec::with_capacity(response.events.len());
    let mut prior_sequence = progress.after;
    for remote in response.events.iter().cloned() {
        let sequence = remote.source.global_sequence;
        if prior_sequence.is_some_and(|prior| sequence <= prior)
            || sequence > response.source_ceiling
            || response.high_watermark.is_none_or(|high| sequence > high)
        {
            return Err(replication_error(
                ReplicationFailureKind::Verification,
                committed_after(progress.imported),
                "remote replica page event order is invalid",
            ));
        }
        prior_sequence = Some(sequence);
        if represented.contains(&remote.source.event_id_hex) {
            progress.deduplicated = progress.deduplicated.saturating_add(1);
            continue;
        }
        let event = substrate::ImportEvent::from_remote(remote).map_err(|error| {
            replication_error(
                ReplicationFailureKind::Verification,
                committed_after(progress.imported),
                format!("remote replica event failed validation: {error}"),
            )
        })?;
        represented.insert(event.source.event_id_hex.clone());
        events.push(event);
    }
    Ok(events)
}

fn finish_remote_import(
    root: &Path,
    circuit: &Circuit,
    endpoint: &str,
    destination: &Store<Open>,
    namespace: String,
    progress: RemoteProgress,
) -> Result<ReplicaReport, TexoError> {
    verify_replica(destination)?;
    let replica_frontier = frontier(destination);
    let cursor = ReplicaCursor {
        schema_version: STATE_SCHEMA_VERSION,
        workspace_id: circuit.workspace.workspace_id.clone(),
        source_journal: circuit.source.id.to_string(),
        replica_journal: circuit.replica.id.to_string(),
        source_namespace: namespace,
        source_endpoint: Some(endpoint.to_string()),
        source_high_watermark: progress.after,
        source_anchor_event_id_hex: progress.after_anchor,
        replica_frontier,
        replica_anchor_event_id_hex: substrate::event_id_at(destination, replica_frontier),
    };
    let path = cursor_path(
        root,
        &circuit.workspace.workspace_id,
        circuit.replica.id.as_str(),
    );
    persist(&path, &cursor, committed_after(progress.imported))?;
    Ok(ReplicaReport::ImportedReadModel {
        imported: progress.imported,
        deduplicated: progress.deduplicated,
        skipped_reserved: progress.skipped_reserved,
        skipped_operational: progress.skipped_operational,
        cursor,
        cursor_path: display_relative(root, &path),
    })
}

fn validate_remote_response(
    circuit: &Circuit,
    after: Option<u64>,
    expected_ceiling: Option<u64>,
    expected_ceiling_anchor: Option<&str>,
    response: &PageResponse,
    imported: u64,
) -> Result<(), TexoError> {
    let binding_matches = response.schema_version == 1
        && response.workspace_id == circuit.workspace.workspace_id
        && response.source_journal == circuit.source.id.as_str();
    let ceiling_matches = expected_ceiling.is_none_or(|ceiling| response.source_ceiling == ceiling);
    let ceiling_anchor_stable = expected_ceiling_anchor.is_none_or(|anchor| {
        response.source_ceiling_anchor_event_id_hex.as_deref() == Some(anchor)
    });
    let ceiling_anchor_matches =
        response.source_ceiling == 0 || response.source_ceiling_anchor_event_id_hex.is_some();
    let cursor_valid = match response.high_watermark {
        Some(high) => {
            after.is_none_or(|previous| high >= previous)
                && high <= response.source_ceiling
                && response.high_watermark_anchor_event_id_hex.is_some()
        }
        None => after.is_none() && response.high_watermark_anchor_event_id_hex.is_none(),
    };
    let completed_at_ceiling = response.has_more
        || response.source_ceiling == 0
        || (response.high_watermark == Some(response.source_ceiling)
            && response.high_watermark_anchor_event_id_hex
                == response.source_ceiling_anchor_event_id_hex);
    if binding_matches
        && ceiling_matches
        && ceiling_anchor_stable
        && ceiling_anchor_matches
        && cursor_valid
        && completed_at_ceiling
    {
        Ok(())
    } else {
        Err(replication_error(
            ReplicationFailureKind::Verification,
            committed_after(imported),
            "remote replica response evidence is inconsistent",
        ))
    }
}

const fn committed_after(imported: u64) -> Committed {
    if imported == 0 {
        Committed::No
    } else {
        Committed::Partial
    }
}
