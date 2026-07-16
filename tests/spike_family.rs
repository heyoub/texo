//! `BatPak` family risk spikes for WO-0.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use batpak::prelude::*;
use batpak::store::AppendPositionHint;
use serde::{Deserialize, Serialize};
use syncbat::{Core, StoreReceiptSink};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ENTITY: &str = "entity:spike";
const SCOPE: &str = "scope:spike";

thread_local! {
    static SPIKE_ENV: RefCell<Option<String>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xF, type_id = 1)]
struct SpikeFact {
    entity: String,
    value: u32,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize, batpak::EventSourced)]
#[batpak(input = RawMsgpackInput, cache_version = 1, state_max_cardinality = 1)]
#[batpak(event = SpikeFact, handler = on_fact)]
struct SpikeState {
    sum: u32,
}

impl SpikeState {
    fn on_fact(&mut self, fact: &SpikeFact) {
        self.sum += fact.value;
    }
}

#[syncbat::operation(
    descriptor = SPIKE_ECHO,
    name = "spike.echo",
    effect = Compute,
    input_schema = "schema.spike.echo.input.v1",
    output_schema = "schema.spike.echo.output.v1",
    receipt_kind = "receipt.spike.echo.v1"
)]
fn spike_echo(input: &[u8], _cx: &mut syncbat::Ctx<'_>) -> syncbat::HandlerResult {
    let prefix = SPIKE_ENV.with(|slot| {
        slot.borrow()
            .clone()
            .unwrap_or_else(|| "missing-env".to_string())
    });
    let mut output = prefix.into_bytes();
    output.extend_from_slice(b":");
    let text = std::str::from_utf8(input)
        .map_err(|error| syncbat::HandlerError::invalid_input(error.to_string()))?;
    output.extend_from_slice(text.as_bytes());
    Ok(output)
}

fn open_store(dir: &TempDir) -> Result<Store, batpak::store::StoreError> {
    Store::open(StoreConfig::new(dir.path()).with_sync_every_n_events(1))
}

fn coord(entity: &str) -> Result<Coordinate, batpak::coordinate::CoordinateError> {
    Coordinate::new(entity, SCOPE)
}

fn fact(value: u32) -> SpikeFact {
    SpikeFact {
        entity: ENTITY.to_string(),
        value,
    }
}

#[test]
fn spike_append_typed_and_receipt_verifies() -> TestResult {
    let dir = TempDir::new()?;
    let store = open_store(&dir)?;
    let coordinate = coord(ENTITY)?;

    let receipt = store.append_typed(&coordinate, &fact(7))?;

    assert!(store.verify_append_receipt(&receipt).is_valid());
    Ok(())
}

#[test]
fn spike_event_sourced_derive_folds_two_events() -> TestResult {
    let dir = TempDir::new()?;
    let store = open_store(&dir)?;
    let coordinate = coord(ENTITY)?;

    let _ = store.append_typed(&coordinate, &fact(2))?;
    let _ = store.append_typed(&coordinate, &fact(5))?;

    let state = store
        .project::<SpikeState>(ENTITY, &Freshness::Consistent)?
        .expect("projection has state");
    assert_eq!(state.sum, 7);
    Ok(())
}

#[test]
fn spike_operation_macro_and_thread_local_env() -> TestResult {
    let dir = TempDir::new()?;
    let store = Arc::new(open_store(&dir)?);
    let receipt_entity = "entity:syncbat-receipts";
    let receipt_coord = coord(receipt_entity)?;
    let receipt_sink = StoreReceiptSink::new(Arc::clone(&store), receipt_coord);

    SPIKE_ENV.with(|slot| {
        *slot.borrow_mut() = Some("env-ok".to_string());
    });

    let mut builder = Core::builder();
    builder.register(SPIKE_ECHO, spike_echo)?;
    builder.receipt_sink(receipt_sink);
    let mut core = builder.build()?;

    let result = core.invoke("spike.echo", b"payload".to_vec())?;

    assert_eq!(result.output(), b"env-ok:payload");
    assert_eq!(store.by_entity(receipt_entity).len(), 1);
    Ok(())
}

#[test]
fn spike_arc_store_drop_flushes() -> TestResult {
    let dir = TempDir::new()?;
    let config = StoreConfig::new(dir.path()).with_sync_every_n_events(1000);
    let store = Arc::new(Store::open(config.clone())?);
    let first = Arc::clone(&store);
    let second = Arc::clone(&store);
    let coordinate = coord(ENTITY)?;

    let _ = store.append_typed(&coordinate, &fact(1))?;
    let _ = first.append_typed(&coordinate, &fact(2))?;
    let _ = second.append_typed(&coordinate, &fact(3))?;
    drop(first);
    drop(second);
    drop(store);

    let reopened = Store::open(config)?;
    assert_eq!(reopened.by_entity(ENTITY).len(), 3);
    Ok(())
}

#[test]
fn spike_lane_hidden_then_promoted() -> TestResult {
    let dir = TempDir::new()?;
    let store = open_store(&dir)?;
    let coordinate = coord(ENTITY)?;

    // Lane write API: AppendOptions::with_position_hint + AppendPositionHint::branch_root.
    let _ = store.append_typed_with_options(
        &coordinate,
        &fact(10),
        AppendOptions::new().with_position_hint(AppendPositionHint::branch_root(1, 0)),
    )?;

    assert!(
        store.stream_lane(ENTITY, 0).is_empty(),
        "non-zero lane write must stay out of the default lane"
    );
    assert_eq!(
        store.query_lane(&Region::entity(ENTITY), 1).len(),
        1,
        "Store::query_lane exposes the non-zero lane"
    );

    let fence = store.begin_visibility_fence()?;
    let mut outbox = fence.outbox();
    // Promotion APIs: Store::begin_visibility_fence, VisibilityFence::outbox,
    // Outbox::stage_typed_with_options, VisibilityFence::commit.
    outbox.stage_typed_with_options(
        coordinate,
        &fact(11),
        AppendOptions::new().with_position_hint(AppendPositionHint::new(1, 1)),
    )?;
    let ticket = outbox.submit_flush()?;
    assert_eq!(
        store.by_entity(ENTITY).len(),
        1,
        "fenced lane write must stay hidden before commit"
    );

    fence.commit()?;
    let receipts = ticket.receiver().recv_timeout(Duration::from_secs(2))??;
    assert_eq!(receipts.len(), 1);
    assert_eq!(store.by_entity(ENTITY).len(), 2);
    Ok(())
}

#[test]
fn spike_project_if_changed_generation() -> TestResult {
    let dir = TempDir::new()?;
    let store = open_store(&dir)?;
    let coordinate = coord(ENTITY)?;

    let _ = store.append_typed(&coordinate, &fact(1))?;
    let initial = store
        .project::<SpikeState>(ENTITY, &Freshness::Consistent)?
        .expect("initial projection has state");
    assert_eq!(initial.sum, 1);
    let generation = store.entity_generation(ENTITY).expect("entity generation");

    let unchanged =
        store.project_if_changed::<SpikeState>(ENTITY, generation, &Freshness::Consistent)?;
    assert!(unchanged.is_none());

    let _ = store.append_typed(&coordinate, &fact(4))?;
    let changed = store
        .project_if_changed::<SpikeState>(ENTITY, generation, &Freshness::Consistent)?
        .expect("projection changed");
    assert!(changed.0 > generation);
    assert_eq!(changed.1.expect("changed projection has state").sum, 5);
    Ok(())
}
