//! Canonical inventory of Texo event payload schemas.

use batpak::event::{EventKind, EventPayload};

use crate::events::payloads::{
    ClaimEvidenceLinkedV1, ClaimRecordedV2, ClaimSupersededV2, CodeIndexRecordedV1,
    ConflictOpenedV2, ConflictResolvedV2, EvidenceOccurrenceRecordedV1,
    EvidenceReconciliationAcceptedV1, OnboardingCompiledV2, RelationCampaignCheckpointV1,
    RelationDeferredV1, RelationJudgedV1, ReplicaBatchMaterializedV1, SessionTurnV1,
    SourceObservedV2, SourceSnapshotRecordedV1, SourceSnapshotRelationV1, WorkspaceInitializedV2,
};

/// One typed event payload binding exposed through `hostbat`.
pub(crate) struct EventSchema {
    /// `BatPak` event kind owned by the payload type.
    pub(crate) kind: EventKind,
    /// Stable schema identifier mounted into the host module.
    pub(crate) schema_ref: &'static str,
    rust_type: fn() -> &'static str,
}

impl EventSchema {
    /// Fully qualified Rust type name for host diagnostics.
    pub(crate) fn rust_type(&self) -> &'static str {
        (self.rust_type)()
    }
}

const fn event_schema<T: EventPayload>(
    schema_ref: &'static str,
    rust_type: fn() -> &'static str,
) -> EventSchema {
    EventSchema {
        kind: T::KIND,
        schema_ref,
        rust_type,
    }
}

fn rust_type<T>() -> &'static str {
    std::any::type_name::<T>()
}

/// Complete, stable event payload surface mounted by Texo's host module.
pub(crate) const EVENT_SCHEMAS: &[EventSchema] = &[
    event_schema::<ClaimRecordedV2>("texo.event.claim-recorded.v2", rust_type::<ClaimRecordedV2>),
    event_schema::<ClaimSupersededV2>(
        "texo.event.claim-superseded.v2",
        rust_type::<ClaimSupersededV2>,
    ),
    event_schema::<ConflictOpenedV2>(
        "texo.event.conflict-opened.v2",
        rust_type::<ConflictOpenedV2>,
    ),
    event_schema::<ConflictResolvedV2>(
        "texo.event.conflict-resolved.v2",
        rust_type::<ConflictResolvedV2>,
    ),
    event_schema::<SourceObservedV2>(
        "texo.event.source-observed.v2",
        rust_type::<SourceObservedV2>,
    ),
    event_schema::<OnboardingCompiledV2>(
        "texo.event.onboarding-compiled.v2",
        rust_type::<OnboardingCompiledV2>,
    ),
    event_schema::<WorkspaceInitializedV2>(
        "texo.event.workspace-initialized.v2",
        rust_type::<WorkspaceInitializedV2>,
    ),
    event_schema::<RelationJudgedV1>(
        "texo.event.relation-judged.v1",
        rust_type::<RelationJudgedV1>,
    ),
    event_schema::<RelationDeferredV1>(
        "texo.event.relation-deferred.v1",
        rust_type::<RelationDeferredV1>,
    ),
    event_schema::<SourceSnapshotRecordedV1>(
        "texo.event.source-snapshot-recorded.v1",
        rust_type::<SourceSnapshotRecordedV1>,
    ),
    event_schema::<EvidenceOccurrenceRecordedV1>(
        "texo.event.evidence-occurrence-recorded.v1",
        rust_type::<EvidenceOccurrenceRecordedV1>,
    ),
    event_schema::<EvidenceReconciliationAcceptedV1>(
        "texo.event.evidence-reconciliation-accepted.v1",
        rust_type::<EvidenceReconciliationAcceptedV1>,
    ),
    event_schema::<ClaimEvidenceLinkedV1>(
        "texo.event.claim-evidence-linked.v1",
        rust_type::<ClaimEvidenceLinkedV1>,
    ),
    event_schema::<CodeIndexRecordedV1>(
        "texo.event.code-index-recorded.v1",
        rust_type::<CodeIndexRecordedV1>,
    ),
    event_schema::<SourceSnapshotRelationV1>(
        "texo.event.source-snapshot-relation.v1",
        rust_type::<SourceSnapshotRelationV1>,
    ),
    event_schema::<SessionTurnV1>("texo.event.session-turn.v1", rust_type::<SessionTurnV1>),
    event_schema::<ReplicaBatchMaterializedV1>(
        "texo.event.replica-batch-materialized.v1",
        rust_type::<ReplicaBatchMaterializedV1>,
    ),
    event_schema::<RelationCampaignCheckpointV1>(
        "texo.event.relation-campaign-checkpoint.v1",
        rust_type::<RelationCampaignCheckpointV1>,
    ),
];
