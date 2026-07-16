//! Texo syncbat effect backend.

use batpak::event::EventKind;
use batpak::id::EntityIdType as _;
use batpak::store::AppendReceipt;
use syncbat::{EffectBackend, EffectError};

use crate::error::TexoError;
use crate::events::coordinate::scope_for_workspace;
use crate::ops::env::{self, OpEnv, ReceiptNote};

mod effect;
mod policy;

pub use effect::TexoEffectBackend;

impl EffectBackend for TexoEffectBackend {
    fn append_event(&mut self, kind: EventKind, payload: &[u8]) -> Result<(), EffectError> {
        with_env_result(|op_env| append_domain_event(op_env, kind, payload))
    }

    fn read_event(&mut self, _event_category: &str) -> Result<(), EffectError> {
        with_env_result(effect_probe)
    }

    fn query_projection(&mut self, _projection_id: &str) -> Result<(), EffectError> {
        with_env_result(effect_probe)
    }
}

fn effect_probe(op_env: &OpEnv) -> Result<(), TexoError> {
    let scope = scope_for_workspace(&op_env.workspace_id);
    let region = batpak::coordinate::Region::scope(&scope);
    if let Some(entry) = op_env.store.query_entries_after(&region, None, 1).first() {
        let _ = op_env.store.read_raw(entry.event_id())?;
    }
    Ok(())
}

fn with_env_result<T>(f: impl FnOnce(&OpEnv) -> Result<T, TexoError>) -> Result<T, EffectError> {
    env::with(f)
        .map_err(|error| effect_error(&error))?
        .map_err(|error| effect_error(&error))
}

fn append_domain_event(
    op_env: &OpEnv,
    kind: EventKind,
    payload_bytes: &[u8],
) -> Result<(), TexoError> {
    let receipt = policy::append(op_env, kind, payload_bytes)?;
    verify_and_note(op_env, kind, &receipt)
}

fn verify_and_note(
    op_env: &OpEnv,
    kind: EventKind,
    receipt: &AppendReceipt,
) -> Result<(), TexoError> {
    let verification = op_env.store.verify_append_receipt(receipt);
    if !verification.is_valid() {
        return Err(TexoError::ReceiptInvalid {
            event_id: event_id_hex(receipt.event_id),
            reason: verification.error().map_or_else(
                || "invalid receipt".to_string(),
                |error| format!("{error:?}"),
            ),
        });
    }

    op_env.receipts.borrow_mut().push(ReceiptNote {
        event_id_hex: event_id_hex(receipt.event_id),
        kind_bits: kind.as_raw_u16(),
        global_sequence: receipt.global_sequence,
    });
    Ok(())
}

fn event_id_hex(event_id: batpak::id::EventId) -> String {
    format!("{:032x}", event_id.as_u128())
}

impl From<batpak::coordinate::CoordinateError> for TexoError {
    fn from(error: batpak::coordinate::CoordinateError) -> Self {
        Self::Coordinate {
            detail: error.to_string(),
        }
    }
}

fn effect_error(error: &TexoError) -> EffectError {
    EffectError::new(format!("{}: {error}", error.code()))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::Arc;

    use batpak::event::EventPayload;
    use batpak::id::{EntityIdType as _, IdempotencyKey};
    use batpak::store::{AppendOptions, Freshness, Store, StoreConfig};

    use super::*;
    use crate::claims::campaign::CampaignCard;
    use crate::claims::workspace::WorkspaceCache;
    use crate::config::WorkspaceConfig;
    use crate::events::coordinate::{coordinate_for_claim, entity_for_relation_campaign};
    use crate::events::ids::{relation_pair_id, ClaimId, WorkspaceId};
    use crate::events::machines::record_claim;
    use crate::events::payloads::{
        ClaimRecordedV2, RelationCampaignCheckpointV1, RelationJudgedV1, SourceObservedV2,
    };
    use crate::relate::settlement::{CampaignPhase, SettledRelation};

    fn test_host_interface() -> crate::host::HostInterface {
        crate::host::HostInterface {
            schema: "hostbat.interface.v1".to_string(),
            version: "test".to_string(),
            fingerprints: crate::host::HostFingerprints {
                module_digest: "00".repeat(32),
                host_fingerprint: "00".repeat(32),
                interface_fingerprint: "00".repeat(32),
            },
            operations: Vec::new(),
        }
    }

    fn test_journal() -> crate::topology::ResolvedJournal {
        crate::config::TexoRootConfig::demo()
            .resolve_journal(Some("demo"), None)
            .expect("test journal")
            .1
    }

    fn test_env(root: &tempfile::TempDir, store: Arc<Store>) -> OpEnv {
        OpEnv {
            store: crate::journal_store::JournalStore::writable(store),
            workspace_id: "demo".to_string(),
            root: root.path().to_path_buf(),
            config: WorkspaceConfig::demo(),
            cache: RefCell::new(WorkspaceCache::default()),
            receipts: RefCell::new(Vec::new()),
            observed_at_ms: 1,
            host_interface: test_host_interface(),
            journal: test_journal(),
        }
    }

    fn campaign_checkpoint(
        phase: CampaignPhase,
        observed_at_ms: u64,
    ) -> RelationCampaignCheckpointV1 {
        RelationCampaignCheckpointV1 {
            workspace_id: WorkspaceId::try_from("demo").expect("workspace"),
            evaluated_basis_digest_hex: "a".repeat(64),
            result_basis_digest_hex: "a".repeat(64),
            candidate_policy_digest_hex: "b".repeat(64),
            phase,
            observed_at_ms,
        }
    }

    fn append_campaign_checkpoint(env: &OpEnv, payload: &RelationCampaignCheckpointV1) {
        let bytes = batpak::canonical::to_bytes(payload).expect("canonical payload");
        append_domain_event(
            env,
            <RelationCampaignCheckpointV1 as EventPayload>::KIND,
            &bytes,
        )
        .expect("append campaign checkpoint");
    }

    #[test]
    fn keyed_source_retry_returns_original_event_and_appends_once() {
        let root = tempfile::tempdir().expect("tempdir");
        let store =
            Arc::new(Store::open(StoreConfig::new(root.path().join("store"))).expect("store"));
        let env = test_env(&root, Arc::clone(&store));
        let payload = SourceObservedV2 {
            source_id: "src_aaaaaaaaaaaa".to_string(),
            workspace_id: "demo".to_string(),
            source_kind: "markdown".to_string(),
            path: "docs/a.md".to_string(),
            body_hash_hex: "body".to_string(),
            observed_at_ms: 1,
        };
        let bytes = batpak::canonical::to_bytes(&payload).expect("canonical payload");
        append_domain_event(&env, <SourceObservedV2 as EventPayload>::KIND, &bytes).expect("first");
        append_domain_event(&env, <SourceObservedV2 as EventPayload>::KIND, &bytes).expect("retry");

        assert_eq!(store.by_entity("source:src_aaaaaaaaaaaa").len(), 1);
        let receipts = env.receipts.borrow();
        assert_eq!(receipts.len(), 2);
        assert_eq!(receipts[0], receipts[1]);
        let expected = IdempotencyKey::for_operation("texo.source.observed.v2", &["demo", "body"]);
        assert_eq!(
            receipts[0].event_id_hex,
            format!("{:032x}", expected.as_u128())
        );
    }

    #[test]
    fn keyed_judgment_retry_returns_original_event_id() {
        let root = tempfile::tempdir().expect("tempdir");
        let store =
            Arc::new(Store::open(StoreConfig::new(root.path().join("store"))).expect("store"));
        let env = test_env(&root, Arc::clone(&store));
        let payload = RelationJudgedV1 {
            workspace_id: WorkspaceId::try_from("demo").expect("workspace"),
            older_claim: ClaimId::try_from("claim_aaaaaaaaaaaa").expect("older"),
            newer_claim: ClaimId::try_from("claim_bbbbbbbbbbbb").expect("newer"),
            relation: SettledRelation::Supersedes,
            score_ppm: 900_000,
            judge_fingerprint: "openrouter:model|relation-v2".to_string(),
            cache_key_hex: "cache".to_string(),
            observed_at_ms: 1,
        };
        let bytes = batpak::canonical::to_bytes(&payload).expect("canonical payload");
        append_domain_event(&env, <RelationJudgedV1 as EventPayload>::KIND, &bytes).expect("first");
        append_domain_event(&env, <RelationJudgedV1 as EventPayload>::KIND, &bytes).expect("retry");

        let payload_identity = blake3::hash(&bytes).to_hex().to_string();
        let expected = IdempotencyKey::for_operation(
            "texo.relation.judged.v1.payload",
            &[
                "demo",
                "claim_aaaaaaaaaaaa",
                "claim_bbbbbbbbbbbb",
                payload_identity.as_str(),
            ],
        );
        let receipts = env.receipts.borrow();
        assert_eq!(receipts.len(), 2);
        assert_eq!(receipts[0], receipts[1]);
        assert_eq!(
            receipts[0].event_id_hex,
            format!("{:032x}", expected.as_u128())
        );
        let pair_id = relation_pair_id(
            &payload.workspace_id,
            &payload.older_claim,
            &payload.newer_claim,
        );
        assert_eq!(store.by_entity(&format!("relation:{pair_id}")).len(), 1);
    }

    #[test]
    fn complete_checkpoint_after_partial_is_a_new_idempotent_transition() {
        let root = tempfile::tempdir().expect("tempdir");
        let store =
            Arc::new(Store::open(StoreConfig::new(root.path().join("store"))).expect("store"));
        let env = test_env(&root, Arc::clone(&store));
        let complete = campaign_checkpoint(CampaignPhase::Complete, 1);
        let partial = campaign_checkpoint(
            CampaignPhase::Partial {
                next_candidate_cursor: 7,
            },
            2,
        );

        append_campaign_checkpoint(&env, &complete);
        append_campaign_checkpoint(&env, &complete);
        append_campaign_checkpoint(&env, &partial);
        append_campaign_checkpoint(&env, &complete);
        append_campaign_checkpoint(&env, &complete);

        let entity = entity_for_relation_campaign("demo");
        let entries = store.by_entity(&entity);
        assert_eq!(entries.len(), 3);
        let receipts = env.receipts.borrow();
        assert_eq!(receipts[0], receipts[1]);
        assert_ne!(receipts[0], receipts[3]);
        assert_eq!(receipts[3], receipts[4]);
        let resumed = store
            .read_raw(entries[2].event_id())
            .expect("resumed checkpoint");
        assert_eq!(
            resumed.event.header.causation_id.map(|id| id.as_u128()),
            Some(entries[1].event_id().as_u128())
        );
        let card = store
            .project::<CampaignCard>(&entity, &Freshness::Consistent)
            .expect("campaign projection")
            .expect("campaign card");
        assert_eq!(card.latest, Some(complete));
    }

    #[test]
    fn typed_options_transition_preserves_payload_bytes() {
        let left_dir = tempfile::tempdir().expect("left tempdir");
        let right_dir = tempfile::tempdir().expect("right tempdir");
        let left = Store::open(StoreConfig::new(left_dir.path())).expect("left store");
        let right = Store::open(StoreConfig::new(right_dir.path())).expect("right store");
        let coordinate = coordinate_for_claim("demo", "claim_aaaaaaaaaaaa").expect("coordinate");
        let payload = ClaimRecordedV2 {
            claim_id: "claim_aaaaaaaaaaaa".to_string(),
            workspace_id: "demo".to_string(),
            source_id: "src_aaaaaaaaaaaa".to_string(),
            source_path: "docs/a.md".to_string(),
            line_start: 1,
            line_end: 1,
            char_start: 0,
            char_end: 5,
            text: "claim".to_string(),
            normalized_text: "claim".to_string(),
            subject_hint: None,
            predicate_hint: None,
            object_hint: None,
            confidence_ppm: 1_000_000,
            extractor_kind: "test".to_string(),
            extractor_model: String::new(),
            prompt_version: String::new(),
            observed_at_ms: 1,
        };
        let left_receipt = left
            .apply_transition(&coordinate, record_claim(payload.clone()))
            .expect("transition");
        let typed_payload = record_claim(payload).into_payload();
        let right_receipt = right
            .append_typed_with_options(
                &coordinate,
                &typed_payload,
                AppendOptions::new().with_idempotency(IdempotencyKey::for_operation(
                    "test",
                    &["claim_aaaaaaaaaaaa"],
                )),
            )
            .expect("typed options");
        let left_raw = left.read_raw(left_receipt.event_id).expect("left raw");
        let right_raw = right.read_raw(right_receipt.event_id).expect("right raw");
        assert_eq!(
            left_raw.event.header.event_kind,
            right_raw.event.header.event_kind
        );
        assert_eq!(left_raw.event.payload, right_raw.event.payload);
    }
}
