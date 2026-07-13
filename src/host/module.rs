//! Content-identified `hostbat` module for Texo's operation surface.

use std::collections::BTreeSet;

use batpak::event::EventPayload;
use hostbat::{
    DiagnosticRustType, GoldenVector, GuardDescriptor, HostModule, HostModuleBuilder,
    SchemaDescriptor, SchemaId, SchemaRole, SchemaVersion,
};
use syncbat::{AdmissionDecision, OperationDescriptor};

use crate::error::TexoError;
use crate::events::payloads::{
    ClaimEvidenceLinkedV1, ClaimRecordedV2, ClaimSupersededV2, CodeIndexRecordedV1,
    ConflictOpenedV2, ConflictResolvedV2, EvidenceOccurrenceRecordedV1,
    EvidenceReconciliationAcceptedV1, OnboardingCompiledV2, RelationDeferredV1, RelationJudgedV1,
    SessionTurnV1, SourceObservedV2, SourceSnapshotRecordedV1, SourceSnapshotRelationV1,
    WorkspaceInitializedV2,
};
use crate::topology::JournalRole;

/// Stable module id folded into the `hostbat` composition fingerprint.
pub const MODULE_ID: &str = "texo.domain";
/// Module contract version. Bump when the mounted operation/schema surface changes.
pub const MODULE_VERSION: u32 = 1;

/// Build the sealed Texo module from the same operation catalog used at runtime.
///
/// # Errors
/// Returns [`TexoError::Host`] when a descriptor, schema, binding, or module
/// invariant fails closed.
pub fn build(role: JournalRole) -> Result<HostModule, TexoError> {
    let catalog = crate::ops::catalog();
    let descriptors = catalog
        .iter()
        .map(|item| item.descriptor().clone())
        .collect::<Vec<_>>();
    let mut builder = HostModule::builder(MODULE_ID, MODULE_VERSION);
    builder = builder
        .guard(
            GuardDescriptor::new("texo.journal-role.v1"),
            move |descriptor: &OperationDescriptor, _input: &[u8], _cx: &mut syncbat::Ctx<'_>| {
                let writes = !descriptor.effect_row().appends_events().is_empty()
                    || matches!(descriptor.effect.as_str(), "persist" | "emit" | "control");
                if role == JournalRole::Replica && writes {
                    AdmissionDecision::deny(
                        "journal.read_only_replica",
                        "authority-bearing operations must target a canonical journal",
                    )
                } else {
                    AdmissionDecision::Admit
                }
            },
        )
        .map_err(host_error)?;
    builder = declare_operation_schemas(builder, &descriptors)?;
    builder = declare_event_payloads(builder)?;
    for item in catalog {
        let (descriptor, handler) = item.into_parts();
        builder = builder.operation(descriptor, handler).map_err(host_error)?;
    }
    builder.build().map_err(host_error)
}

fn declare_operation_schemas(
    mut builder: HostModuleBuilder,
    descriptors: &[OperationDescriptor],
) -> Result<HostModuleBuilder, TexoError> {
    let mut declared = BTreeSet::new();
    for descriptor in descriptors {
        for (schema_ref, role) in [
            (descriptor.input_schema_ref(), SchemaRole::OperationInput),
            (descriptor.output_schema_ref(), SchemaRole::OperationOutput),
            (descriptor.receipt_kind(), SchemaRole::ReceiptPayload),
        ] {
            if declared.insert((schema_ref.to_string(), role)) {
                builder = builder
                    .schema(schema_descriptor(schema_ref, role, None)?)
                    .map_err(host_error)?;
            }
        }
    }
    Ok(builder)
}

fn declare_event_payloads(mut builder: HostModuleBuilder) -> Result<HostModuleBuilder, TexoError> {
    macro_rules! bind {
        ($payload:ty, $schema:literal) => {{
            builder = builder
                .schema(schema_descriptor(
                    $schema,
                    SchemaRole::EventPayload,
                    Some(std::any::type_name::<$payload>()),
                )?)
                .map_err(host_error)?;
            builder = builder
                .bind_event_payload(<$payload as EventPayload>::KIND, $schema)
                .map_err(host_error)?;
        }};
    }

    bind!(ClaimRecordedV2, "texo.event.claim-recorded.v2");
    bind!(ClaimSupersededV2, "texo.event.claim-superseded.v2");
    bind!(ConflictOpenedV2, "texo.event.conflict-opened.v2");
    bind!(ConflictResolvedV2, "texo.event.conflict-resolved.v2");
    bind!(SourceObservedV2, "texo.event.source-observed.v2");
    bind!(OnboardingCompiledV2, "texo.event.onboarding-compiled.v2");
    bind!(
        WorkspaceInitializedV2,
        "texo.event.workspace-initialized.v2"
    );
    bind!(RelationJudgedV1, "texo.event.relation-judged.v1");
    bind!(RelationDeferredV1, "texo.event.relation-deferred.v1");
    bind!(
        SourceSnapshotRecordedV1,
        "texo.event.source-snapshot-recorded.v1"
    );
    bind!(
        EvidenceOccurrenceRecordedV1,
        "texo.event.evidence-occurrence-recorded.v1"
    );
    bind!(
        EvidenceReconciliationAcceptedV1,
        "texo.event.evidence-reconciliation-accepted.v1"
    );
    bind!(ClaimEvidenceLinkedV1, "texo.event.claim-evidence-linked.v1");
    bind!(CodeIndexRecordedV1, "texo.event.code-index-recorded.v1");
    bind!(
        SourceSnapshotRelationV1,
        "texo.event.source-snapshot-relation.v1"
    );
    bind!(SessionTurnV1, "texo.event.session-turn.v1");
    Ok(builder)
}

fn schema_descriptor(
    schema_ref: &str,
    role: SchemaRole,
    rust_type: Option<&str>,
) -> Result<SchemaDescriptor, TexoError> {
    // The typed handlers/backends are the load-bearing shape checks. This
    // committed vector pins the canonical map wire dialect at the host seam;
    // per-operation and per-event decoding then validates the concrete type.
    let golden = batpak::canonical::to_bytes(&std::collections::BTreeMap::<String, String>::new())
        .map_err(host_error)?;
    let mut descriptor = SchemaDescriptor::new(
        SchemaId::new(schema_ref).map_err(host_error)?,
        SchemaVersion(schema_version(schema_ref)),
        role,
        vec![GoldenVector::new("empty-map", golden)],
    )
    .map_err(host_error)?;
    if let Some(rust_type) = rust_type {
        descriptor = descriptor.with_diagnostic_rust_type(DiagnosticRustType::new(rust_type));
    }
    Ok(descriptor)
}

fn schema_version(schema_ref: &str) -> u32 {
    schema_ref
        .rsplit_once(".v")
        .and_then(|(_, version)| version.parse().ok())
        .unwrap_or(1)
}

fn host_error(error: impl std::fmt::Display) -> TexoError {
    TexoError::Host {
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_seals_the_complete_catalog_and_event_surface() {
        let module = build(JournalRole::Canonical).expect("module");
        assert!(module.manifest().verify_hash().expect("manifest hash"));
        assert_eq!(
            module.manifest().operations().count(),
            crate::ops::catalog().len()
        );
        assert_eq!(module.manifest().event_payload_bindings().count(), 16);
    }

    #[test]
    fn every_declared_append_kind_has_one_payload_binding() {
        let module = build(JournalRole::Canonical).expect("module");
        let bound = module
            .manifest()
            .event_payload_bindings()
            .map(|binding| format!("evt.{:04x}", binding.kind_raw()))
            .collect::<BTreeSet<_>>();
        let declared = crate::ops::catalog()
            .into_iter()
            .flat_map(|item| item.descriptor().effect_row().appends_events().to_vec())
            .collect::<BTreeSet<_>>();
        assert_eq!(bound, declared);
    }
}
