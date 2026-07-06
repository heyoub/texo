//! Canonical operation interface fingerprints.

use serde::Serialize;
use syncbat::{OperationDescriptor, OperationRegisterItem};

/// Canonical fingerprint schema name.
pub const SCHEMA: &str = "texo-canonical-v1";

/// One operation row exposed in the canonical interface output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CanonicalOperation {
    /// Stable operation name.
    pub name: String,
    /// Stable effect class spelling.
    pub effect: String,
    /// Stable receipt kind.
    pub receipt_kind: String,
}

/// Canonical operation interface output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CanonicalInterface {
    /// Fingerprint schema identifier.
    pub schema: String,
    /// Blake3 digest over sorted operation descriptor lines.
    pub interface_fingerprint: String,
    /// Number of operations included in the digest.
    pub operation_count: usize,
    /// Sorted public operation summary.
    pub operations: Vec<CanonicalOperation>,
}

/// Build the canonical interface output for an operation catalog.
#[must_use]
pub fn canonical_interface(catalog: &[OperationRegisterItem]) -> CanonicalInterface {
    let descriptors = catalog
        .iter()
        .map(|item| item.descriptor().clone())
        .collect::<Vec<_>>();
    canonical_interface_for_descriptors(&descriptors)
}

/// Build the canonical interface output for operation descriptors.
#[must_use]
pub fn canonical_interface_for_descriptors(
    descriptors: &[OperationDescriptor],
) -> CanonicalInterface {
    let mut rows = descriptors.iter().map(descriptor_line).collect::<Vec<_>>();
    rows.sort();
    let joined = rows.join("\n");
    let interface_fingerprint = blake3::hash(joined.as_bytes()).to_hex().to_string();
    let mut operations = descriptors
        .iter()
        .map(|descriptor| CanonicalOperation {
            name: descriptor.name().to_string(),
            effect: descriptor.effect.as_str().to_string(),
            receipt_kind: descriptor.receipt_kind().to_string(),
        })
        .collect::<Vec<_>>();
    operations.sort_by(|left, right| left.name.cmp(&right.name));
    CanonicalInterface {
        schema: SCHEMA.to_string(),
        interface_fingerprint,
        operation_count: operations.len(),
        operations,
    }
}

fn descriptor_line(descriptor: &OperationDescriptor) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        descriptor.name(),
        descriptor.effect.as_str(),
        descriptor.input_schema_ref(),
        descriptor.output_schema_ref(),
        descriptor.receipt_kind()
    )
}

#[cfg(test)]
mod tests {
    use syncbat::{EffectClass, OperationDescriptor};

    use super::*;

    #[test]
    fn fingerprint_is_stable_across_catalog_constructions() {
        let first = canonical_interface(&crate::ops::catalog());
        let second = canonical_interface(&crate::ops::catalog());
        assert_eq!(first.interface_fingerprint, second.interface_fingerprint);
        assert_eq!(first.operations, second.operations);
    }

    #[test]
    fn fingerprint_changes_when_probe_descriptor_is_added() {
        let mut descriptors = crate::ops::catalog()
            .into_iter()
            .map(|item| item.descriptor().clone())
            .collect::<Vec<_>>();
        let baseline = canonical_interface_for_descriptors(&descriptors);
        descriptors.push(OperationDescriptor::new(
            "texo.probe",
            EffectClass::Inspect,
            "texo.probe.input.v1",
            "texo.probe.output.v1",
            "receipt.texo.probe.v1",
        ));
        let changed = canonical_interface_for_descriptors(&descriptors);
        assert_ne!(
            baseline.interface_fingerprint,
            changed.interface_fingerprint
        );
    }
}
