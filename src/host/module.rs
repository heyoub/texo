//! Content-identified `hostbat` module for Texo's operation surface.

use std::collections::BTreeSet;

use hostbat::{
    DiagnosticRustType, GoldenVector, GuardDescriptor, HostModule, HostModuleBuilder,
    SchemaDescriptor, SchemaId, SchemaRole, SchemaVersion,
};
use syncbat::{AdmissionDecision, OperationDescriptor};

use crate::error::TexoError;
use crate::events::inventory::EVENT_SCHEMAS;
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
    for event in EVENT_SCHEMAS {
        builder = builder
            .schema(schema_descriptor(
                event.schema_ref,
                SchemaRole::EventPayload,
                Some(event.rust_type()),
            )?)
            .map_err(host_error)?;
        builder = builder
            .bind_event_payload(event.kind, event.schema_ref)
            .map_err(host_error)?;
    }
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
    use batpak::event::EventPayload;

    use super::*;
    use crate::events::payloads::ReplicaBatchMaterializedV1;

    #[test]
    fn module_seals_the_complete_catalog_and_event_surface() {
        let module = build(JournalRole::Canonical).expect("module");
        assert!(module.manifest().verify_hash().expect("manifest hash"));
        assert_eq!(
            module.manifest().operations().count(),
            crate::ops::catalog().len()
        );
        assert_eq!(
            module.manifest().event_payload_bindings().count(),
            EVENT_SCHEMAS.len()
        );
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
        assert!(declared.is_subset(&bound));
        assert!(bound.contains(&format!(
            "evt.{:04x}",
            ReplicaBatchMaterializedV1::KIND.as_raw_u16()
        )));
    }
}
