//! Rejudge append idempotency and reopen-level settlement durability.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use batpak::event::EventPayload;
use batpak::store::{Freshness, Store, StoreConfig};
use syncbat::EffectBackend;
use tempfile::TempDir;
use texo::claims::settlement::SettlementCard;
use texo::claims::workspace::WorkspaceCache;
use texo::config::WorkspaceConfig;
use texo::events::coordinate::entity_for_relation_pair;
use texo::events::ids::{relation_pair_id, ClaimId, WorkspaceId};
use texo::events::payloads::RelationJudgedV1;
use texo::host::{HostFingerprints, HostInterface};
use texo::journal_store::JournalStore;
use texo::ops::backend::TexoEffectBackend;
use texo::ops::env::{self, OpEnv};
use texo::relate::settlement::SettledRelation;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn test_env(root: &TempDir, store: Arc<Store>) -> Rc<OpEnv> {
    Rc::new(OpEnv {
        store: JournalStore::writable(store),
        workspace_id: "demo".to_string(),
        root: root.path().to_path_buf(),
        config: WorkspaceConfig::demo(),
        cache: RefCell::new(WorkspaceCache::default()),
        receipts: RefCell::new(Vec::new()),
        observed_at_ms: 1,
        host_interface: HostInterface {
            schema: "hostbat.interface.v1".to_string(),
            version: "test".to_string(),
            fingerprints: HostFingerprints {
                module_digest: "00".repeat(32),
                host_fingerprint: "00".repeat(32),
                interface_fingerprint: "00".repeat(32),
            },
            operations: Vec::new(),
        },
        journal: texo::config::TexoRootConfig::demo()
            .resolve_journal(Some("demo"), None)
            .expect("test journal")
            .1,
    })
}

fn append_judgment(backend: &mut TexoEffectBackend, payload: &RelationJudgedV1) -> TestResult {
    let bytes = batpak::canonical::to_bytes(payload)?;
    backend.append_event(RelationJudgedV1::KIND, &bytes)?;
    Ok(())
}

#[test]
fn changed_same_fingerprint_judgment_survives_reopen_as_later_history() -> TestResult {
    let root = TempDir::new()?;
    let store_path = root.path().join("store");
    let store = Arc::new(Store::open(StoreConfig::new(&store_path))?);
    let op_env = test_env(&root, Arc::clone(&store));
    let guard = env::install(Rc::clone(&op_env));
    let mut backend = TexoEffectBackend;
    let workspace = WorkspaceId::try_from("demo")?;
    let older = ClaimId::try_from("claim_aaaaaaaaaaaa")?;
    let newer = ClaimId::try_from("claim_bbbbbbbbbbbb")?;
    let first = RelationJudgedV1 {
        workspace_id: workspace.clone(),
        older_claim: older.clone(),
        newer_claim: newer.clone(),
        relation: SettledRelation::Supersedes,
        score_ppm: 900_000,
        judge_fingerprint: "openrouter:model|relation-v2".to_string(),
        cache_key_hex: "first-cache".to_string(),
        observed_at_ms: 1,
    };
    let later = RelationJudgedV1 {
        relation: SettledRelation::Conflicts,
        score_ppm: 750_000,
        cache_key_hex: "later-cache".to_string(),
        observed_at_ms: 2,
        ..first.clone()
    };

    append_judgment(&mut backend, &first)?;
    append_judgment(&mut backend, &first)?;
    append_judgment(&mut backend, &later)?;
    append_judgment(&mut backend, &later)?;

    let receipts = op_env.receipts.borrow();
    assert_eq!(receipts.len(), 4);
    assert_eq!(receipts[0], receipts[1]);
    assert_eq!(receipts[2], receipts[3]);
    assert_ne!(receipts[0].event_id_hex, receipts[2].event_id_hex);
    drop(receipts);

    let pair_id = relation_pair_id(&workspace, &older, &newer);
    let entity = entity_for_relation_pair(pair_id.as_str());
    assert_eq!(store.by_entity(&entity).len(), 2);

    drop(guard);
    drop(op_env);
    let store = Arc::try_unwrap(store).map_err(|_| "test store still has shared owners")?;
    let _closed = store.close()?;
    let reopened = Store::open_read_only(StoreConfig::new(store_path))?;
    assert_eq!(reopened.by_entity(&entity).len(), 2);
    let card = reopened
        .project::<SettlementCard>(&entity, &Freshness::Consistent)?
        .ok_or("settlement card missing after reopen")?;
    assert_eq!(
        card.authoritative.as_ref().map(|row| row.relation),
        Some(SettledRelation::Supersedes)
    );
    assert_eq!(card.later_judgments.len(), 1);
    assert_eq!(card.later_judgments[0].relation, SettledRelation::Conflicts);
    assert_eq!(card.later_judgments[0].cache_key_hex, "later-cache");
    Ok(())
}
