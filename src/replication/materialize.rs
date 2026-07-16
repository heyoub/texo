use std::collections::BTreeSet;

use batpak::coordinate::Coordinate;
use batpak::event::EventPayload;
use batpak::id::IdempotencyKey;
use batpak::store::{AppendOptions, BatchAppendItem, CausationRef, Open, Store};

use crate::compat::batpak as substrate;
use crate::error::{Committed, ReplicationFailureKind, TexoError};
use crate::events::payloads::{ReplicaBatchMaterializedV1, ReplicaSourceEventV1};

use super::evidence::{hex_lower, replication_error};
use super::Circuit;

pub(super) fn verify_replica(destination: &Store<Open>) -> Result<(), TexoError> {
    substrate::verify_intact(destination)
        .map(|_| ())
        .map_err(|error| {
            replication_error(
                ReplicationFailureKind::Verification,
                Committed::Partial,
                error.to_string(),
            )
        })
}

pub(super) fn replica_ledger_item(
    circuit: &Circuit,
    namespace: &str,
    events: &[substrate::ImportEvent],
) -> Result<BatchAppendItem, TexoError> {
    let entries = events
        .iter()
        .map(|event| ReplicaSourceEventV1 {
            source_event_id_hex: event.source.event_id_hex.clone(),
            source_global_sequence: event.source.global_sequence,
            source_kind: event.source.kind_raw,
            source_content_hash_hex: hex_lower(&event.source.content_hash),
        })
        .collect::<Vec<_>>();
    let first = entries.first().map_or_else(
        || "0".to_string(),
        |entry| entry.source_global_sequence.to_string(),
    );
    let last = entries.last().map_or_else(
        || "0".to_string(),
        |entry| entry.source_global_sequence.to_string(),
    );
    let payload = ReplicaBatchMaterializedV1 {
        workspace_id: circuit.workspace.workspace_id.clone(),
        source_journal: circuit.source.id.to_string(),
        replica_journal: circuit.replica.id.to_string(),
        source_namespace: namespace.to_string(),
        events: entries,
    };
    let coordinate =
        replica_ledger_coordinate(&circuit.workspace.workspace_id, circuit.replica.id.as_str())?;
    let options = AppendOptions::new().with_idempotency(IdempotencyKey::for_operation(
        "texo.replica.batch.v1",
        &[
            &circuit.workspace.workspace_id,
            circuit.source.id.as_str(),
            circuit.replica.id.as_str(),
            &first,
            &last,
        ],
    ));
    BatchAppendItem::typed(coordinate, &payload, options, CausationRef::None).map_err(|error| {
        replication_error(
            ReplicationFailureKind::Substrate,
            Committed::No,
            format!("replica ledger encoding failed: {error}"),
        )
    })
}

pub(super) fn represented_source_events(
    destination: &Store<Open>,
    workspace: &str,
    replica: &str,
    namespace: &str,
) -> Result<BTreeSet<String>, TexoError> {
    let coordinate = replica_ledger_coordinate(workspace, replica)?;
    let mut represented = BTreeSet::new();
    for entry in destination.by_entity(coordinate.entity()) {
        if entry.event_kind() != ReplicaBatchMaterializedV1::KIND {
            continue;
        }
        let raw = destination.read_raw(entry.event_id()).map_err(|error| {
            replication_error(
                ReplicationFailureKind::Verification,
                Committed::No,
                format!("replica ledger read failed: {error}"),
            )
        })?;
        let payload: ReplicaBatchMaterializedV1 = batpak::encoding::from_bytes(&raw.event.payload)
            .map_err(|error| {
                replication_error(
                    ReplicationFailureKind::Verification,
                    Committed::No,
                    format!("replica ledger decode failed: {error}"),
                )
            })?;
        if payload.workspace_id != workspace
            || payload.replica_journal != replica
            || payload.source_namespace != namespace
        {
            return Err(replication_error(
                ReplicationFailureKind::Verification,
                Committed::No,
                "replica ledger binding does not match the configured circuit",
            ));
        }
        represented.extend(
            payload
                .events
                .into_iter()
                .map(|source| source.source_event_id_hex),
        );
    }
    Ok(represented)
}

pub(super) fn replica_ledger_coordinate(
    workspace: &str,
    replica: &str,
) -> Result<Coordinate, TexoError> {
    Coordinate::new(
        format!("replica-ledger:{workspace}:{replica}"),
        format!("replication:{workspace}"),
    )
    .map_err(TexoError::from)
}
