//! WO-2a operation kit invariants.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use batpak::coordinate::Coordinate;
use batpak::event::{EventKind, EventPayload};
use batpak::store::{Store, StoreConfig};
use serde_json::json;
use syncbat::{Core, HandlerError, RuntimeError, StoreReceiptSink};
use tempfile::TempDir;
use texo::claims::workspace::WorkspaceCache;
use texo::config::WorkspaceConfig;
use texo::events::payloads::SourceObservedV2;
use texo::host::TexoHost;
use texo::ops::backend::TexoEffectBackend;
use texo::ops::env::{self, OpEnv};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type ScriptedCore = (Core, Rc<OpEnv>, Arc<Store>);
type ScriptedRegister = for<'a> fn(
    &'a mut syncbat::CoreBuilder,
) -> Result<&'a mut syncbat::CoreBuilder, syncbat::BuildError>;

thread_local! {
    static SCRIPTED_PROBE_RAN: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

#[syncbat::operation(
    descriptor = ROW_VIOLATION,
    name = "scripted.row_violation",
    effect = Inspect,
    input_schema = "scripted.row_violation.input.v1",
    output_schema = "scripted.row_violation.output.v1",
    receipt_kind = "receipt.scripted.row_violation.v1"
)]
fn row_violation(_input: &[u8], cx: &mut syncbat::Ctx<'_>) -> syncbat::HandlerResult {
    let payload = source_payload("src_row_violation");
    let bytes = batpak::canonical::to_bytes(&payload)
        .map_err(|error| HandlerError::failed(error.to_string()))?;
    cx.event_append_handle()
        .append_event(<SourceObservedV2 as EventPayload>::KIND, &bytes)
        .map_err(|error| HandlerError::failed(error.to_string()))?;
    Ok(b"{}".to_vec())
}

#[syncbat::operation(
    descriptor = UNKNOWN_KIND,
    name = "scripted.unknown_kind",
    effect = Persist,
    input_schema = "scripted.unknown_kind.input.v1",
    output_schema = "scripted.unknown_kind.output.v1",
    receipt_kind = "receipt.scripted.unknown_kind.v1",
    appends_events = ["evt.f001"]
)]
fn unknown_kind(_input: &[u8], cx: &mut syncbat::Ctx<'_>) -> syncbat::HandlerResult {
    cx.event_append_handle()
        .append_event(EventKind::custom(0xF, 1), b"{}")
        .map_err(|error| HandlerError::failed(error.to_string()))?;
    Ok(b"{}".to_vec())
}

#[syncbat::operation(
    descriptor = CAPABILITY_PROBE,
    name = "scripted.capability_probe",
    effect = Inspect,
    input_schema = "scripted.capability_probe.input.v1",
    output_schema = "scripted.capability_probe.output.v1",
    receipt_kind = "receipt.scripted.capability_probe.v1",
    requires_capabilities = ["texo.cap.model"]
)]
fn capability_probe(input: &[u8], _cx: &mut syncbat::Ctx<'_>) -> syncbat::HandlerResult {
    let _request: serde_json::Value = serde_json::from_slice(input)
        .map_err(|error| HandlerError::invalid_input(error.to_string()))?;
    SCRIPTED_PROBE_RAN.with(|slot| {
        if let Some(flag) = slot.borrow().as_ref() {
            flag.store(true, Ordering::SeqCst);
        }
    });
    Ok(b"{}".to_vec())
}

fn register_row_violation(
    builder: &mut syncbat::CoreBuilder,
) -> Result<&mut syncbat::CoreBuilder, syncbat::BuildError> {
    builder.register(ROW_VIOLATION, row_violation)
}

fn register_unknown_kind(
    builder: &mut syncbat::CoreBuilder,
) -> Result<&mut syncbat::CoreBuilder, syncbat::BuildError> {
    builder.register((*UNKNOWN_KIND).clone(), unknown_kind)
}

fn register_capability_probe(
    builder: &mut syncbat::CoreBuilder,
) -> Result<&mut syncbat::CoreBuilder, syncbat::BuildError> {
    builder.register((*CAPABILITY_PROBE).clone(), capability_probe)
}

fn source_payload(source_id: &str) -> SourceObservedV2 {
    SourceObservedV2 {
        source_id: source_id.to_string(),
        workspace_id: "demo".to_string(),
        source_kind: "markdown".to_string(),
        path: "source.md".to_string(),
        body_hash_hex: blake3::hash(source_id.as_bytes()).to_hex().to_string(),
        observed_at_ms: 1,
    }
}

fn receipt_coord() -> Result<Coordinate, batpak::coordinate::CoordinateError> {
    Coordinate::new("op-receipts:demo", "ops:demo")
}

fn test_env(dir: &TempDir, store: Arc<Store>) -> Rc<OpEnv> {
    Rc::new(OpEnv {
        store: texo::journal_store::JournalStore::writable(store),
        workspace_id: "demo".to_string(),
        root: dir.path().to_path_buf(),
        config: WorkspaceConfig::demo(),
        cache: RefCell::new(WorkspaceCache::default()),
        receipts: RefCell::new(Vec::new()),
        observed_at_ms: 1,
        host_interface: texo::host::HostInterface {
            schema: "hostbat.interface.v1".to_string(),
            version: "test".to_string(),
            fingerprints: texo::host::HostFingerprints {
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

fn scripted_core(
    dir: &TempDir,
    register: ScriptedRegister,
) -> Result<ScriptedCore, Box<dyn std::error::Error>> {
    let store = Arc::new(Store::open(StoreConfig::new(dir.path().join("store")))?);
    let mut builder = Core::builder();
    let _builder = register(&mut builder)?;
    builder.receipt_sink(StoreReceiptSink::new(Arc::clone(&store), receipt_coord()?));
    builder.effect_backend(TexoEffectBackend);
    let core = builder.build()?;
    let op_env = test_env(dir, Arc::clone(&store));
    Ok((core, op_env, store))
}

#[test]
fn effect_row_fail_closed_records_denied_receipt() -> TestResult {
    let dir = TempDir::new()?;
    let (mut core, op_env, store) = scripted_core(&dir, register_row_violation)?;
    let _guard = env::install(op_env);

    let Err(error) = core.invoke("scripted.row_violation", b"{}".to_vec()) else {
        return Err("effect row violation unexpectedly succeeded".into());
    };

    assert!(matches!(error, RuntimeError::Denied { .. }));
    assert_eq!(store.by_scope("ops:demo").len(), 1);
    Ok(())
}

#[test]
fn core_builder_without_receipt_sink_fails() {
    let mut builder = Core::builder();
    register_capability_probe(&mut builder).expect("register probe");
    let result = builder.build();
    assert!(matches!(
        result,
        Err(syncbat::BuildError::MissingReceiptSink)
    ));
}

#[test]
fn unknown_kind_append_fails_and_journals_nothing_in_workspace_scope() -> TestResult {
    let dir = TempDir::new()?;
    let (mut core, op_env, store) = scripted_core(&dir, register_unknown_kind)?;
    let _guard = env::install(op_env);

    let Err(error) = core.invoke("scripted.unknown_kind", b"{}".to_vec()) else {
        return Err("unknown kind unexpectedly succeeded".into());
    };

    assert!(matches!(error, RuntimeError::Handler { .. }));
    assert!(store.by_scope("workspace:demo").is_empty());
    Ok(())
}

#[test]
fn capability_gate_denies_before_handler_runs() -> TestResult {
    let dir = TempDir::new()?;
    let (mut core, op_env, _store) = scripted_core(&dir, register_capability_probe)?;
    let flag = Arc::new(AtomicBool::new(false));
    SCRIPTED_PROBE_RAN.with(|slot| {
        *slot.borrow_mut() = Some(Arc::clone(&flag));
    });
    let _guard = env::install(op_env);

    let Err(error) = core.invoke("scripted.capability_probe", b"{}".to_vec()) else {
        return Err("missing capability unexpectedly succeeded".into());
    };

    assert!(matches!(error, RuntimeError::Denied { .. }));
    assert!(!flag.load(Ordering::SeqCst));
    SCRIPTED_PROBE_RAN.with(|slot| {
        *slot.borrow_mut() = None;
    });
    Ok(())
}

#[test]
fn catalog_descriptors_validate_and_fingerprints_exist() -> TestResult {
    let catalog = texo::ops::catalog();
    for item in &catalog {
        item.descriptor().validate()?;
    }
    let relate = catalog
        .iter()
        .find(|item| item.descriptor().name() == "texo.relate.run")
        .ok_or("relate operation")?;
    let effect_row = relate.descriptor().effect_row();
    assert_eq!(
        effect_row
            .appends_events()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["evt.e003", "evt.e004", "evt.e009", "evt.e00a", "evt.e012"]
    );
    assert!(effect_row
        .requires_capabilities()
        .iter()
        .any(|capability| capability == "texo.cap.model"));
    let dir = TempDir::new()?;
    let host = TexoHost::open(dir.path(), "demo", 1)?;
    let fingerprints = host.fingerprints();
    assert_eq!(fingerprints.module_digest.len(), 64);
    assert_eq!(fingerprints.host_fingerprint.len(), 64);
    assert_eq!(fingerprints.interface_fingerprint.len(), 64);
    Ok(())
}

#[test]
fn courtroom_on_ops() -> TestResult {
    let dir = TempDir::new()?;
    let docs = dir.path().join("docs");
    std::fs::create_dir_all(&docs)?;
    let friday = docs.join("friday.md");
    let tuesday = docs.join("tuesday.md");
    std::fs::write(&friday, "Deploys happen on Friday.\n")?;
    std::fs::write(&tuesday, "Decision: deploys moved to Tuesday.\n")?;

    let mut host = TexoHost::open(dir.path(), "demo", 1)?;
    let init = host.invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))?;
    assert!(init.get("receipt").is_some());

    let first = host.invoke_json(
        "texo.ingest.run",
        &json!({"path": "docs/friday.md", "dry_run": false, "observed_at_ms": 10}),
    )?;
    assert!(first
        .get("receipts")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|receipts| !receipts.is_empty()));
    let second = host.invoke_json(
        "texo.ingest.run",
        &json!({"path": "docs/tuesday.md", "dry_run": false, "observed_at_ms": 20}),
    )?;
    assert!(second
        .get("receipts")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|receipts| !receipts.is_empty()));
    assert_eq!(second["claims_superseded"], 1);
    assert_eq!(second["supersessions_held"], 0);
    assert_eq!(second["held_supersessions"], json!([]));

    let list = host.invoke_json("texo.claims.list", &json!({"subject": null}))?;
    let claims = list
        .get("claims")
        .and_then(serde_json::Value::as_array)
        .expect("claims array");
    let friday_claim = claims
        .iter()
        .find(|claim| {
            claim.get("text").and_then(serde_json::Value::as_str)
                == Some("Deploys happen on Friday.")
        })
        .expect("Friday claim listed");
    let tuesday_claim = claims
        .iter()
        .find(|claim| {
            claim.get("text").and_then(serde_json::Value::as_str)
                == Some("Decision: deploys moved to Tuesday.")
        })
        .expect("Tuesday claim listed");
    assert_eq!(tuesday_claim.get("status"), Some(&json!("current")));
    assert_eq!(friday_claim.get("status"), Some(&json!("superseded")));
    assert_eq!(
        friday_claim.get("superseded_by"),
        tuesday_claim.get("claim_id")
    );

    let friday_id = friday_claim
        .get("claim_id")
        .and_then(serde_json::Value::as_str)
        .expect("Friday claim id");
    let explain = host.invoke_json("texo.claim.explain", &json!({"claim_id": friday_id}))?;
    let timeline = explain
        .get("timeline")
        .and_then(serde_json::Value::as_array)
        .expect("timeline array");
    assert!(timeline
        .iter()
        .any(|entry| entry.get("kind") == Some(&json!("recorded"))));
    assert!(timeline
        .iter()
        .any(|entry| entry.get("kind") == Some(&json!("superseded"))));

    let verify = host.invoke_json("texo.verify.run", &json!({}))?;
    assert_eq!(verify.get("projection_ok"), Some(&json!(true)));
    assert_eq!(verify.get("journal_ok"), Some(&json!(true)));
    assert_eq!(verify.get("transitions_ok"), Some(&json!(true)));
    assert_eq!(
        verify
            .get("errors")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    assert!(!host.store().by_scope("ops:demo").is_empty());
    Ok(())
}
