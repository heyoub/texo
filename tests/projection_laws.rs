//! Projection laws for WO-1 state machines and workspace assembly.

use std::time::Duration;

use batpak::prelude::*;
use batpak::store::AppendPositionHint;
use tempfile::TempDir;
use texo::claims::card::ClaimCard;
use texo::claims::session_log::SessionLog;
use texo::claims::status::ClaimStatus;
use texo::claims::workspace::{assemble, ClaimView, ProjectionFreshness, WorkspaceCache};
use texo::events::coordinate::{
    coordinate_for_claim, coordinate_for_conflict, coordinate_for_session, entity_for_session,
    scope_for_workspace, session_lane,
};
use texo::events::machines::{
    open_conflict, record_claim, supersede_claim, transition_record, CLAIM_MACHINE,
    CONFLICT_MACHINE,
};
use texo::events::payloads::{ClaimRecordedV2, ClaimSupersededV2, ConflictOpenedV2, SessionTurnV1};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const WORKSPACE: &str = "demo";

fn open_store(dir: &TempDir) -> Result<Store, batpak::store::StoreError> {
    Store::open(StoreConfig::new(dir.path()).with_sync_every_n_events(1))
}

fn claim_recorded(claim_id: &str, observed_at_ms: u64) -> ClaimRecordedV2 {
    ClaimRecordedV2 {
        claim_id: claim_id.to_string(),
        workspace_id: WORKSPACE.to_string(),
        source_id: "src_a".to_string(),
        source_path: "docs/a.md".to_string(),
        line_start: 1,
        line_end: 1,
        char_start: 0,
        char_end: 10,
        text: format!("{claim_id} text"),
        normalized_text: format!("{claim_id} text"),
        subject_hint: Some(claim_id.to_string()),
        predicate_hint: Some("says".to_string()),
        object_hint: None,
        confidence_ppm: 900_000,
        extractor_kind: "scripted".to_string(),
        extractor_model: "none".to_string(),
        prompt_version: "v1".to_string(),
        observed_at_ms,
    }
}

fn claim_superseded(
    old_claim_id: &str,
    new_claim_id: &str,
    reason: &str,
    observed_at_ms: u64,
) -> ClaimSupersededV2 {
    ClaimSupersededV2 {
        old_claim_id: old_claim_id.to_string(),
        new_claim_id: new_claim_id.to_string(),
        workspace_id: WORKSPACE.to_string(),
        reason: reason.to_string(),
        decided_by: "human".to_string(),
        observed_at_ms,
        transition: transition_record(
            CLAIM_MACHINE,
            old_claim_id,
            1,
            2,
            Vec::new(),
            observed_at_ms,
        ),
    }
}

fn conflict_opened(
    conflict_id: &str,
    claim_a: &str,
    claim_b: &str,
    observed_at_ms: u64,
) -> ConflictOpenedV2 {
    ConflictOpenedV2 {
        conflict_id: conflict_id.to_string(),
        workspace_id: WORKSPACE.to_string(),
        claim_a: claim_a.to_string(),
        claim_b: claim_b.to_string(),
        reason: "contradiction".to_string(),
        detector: "scripted".to_string(),
        observed_at_ms,
        transition: transition_record(
            CONFLICT_MACHINE,
            conflict_id,
            0,
            1,
            Vec::new(),
            observed_at_ms,
        ),
    }
}

fn session_turn(session_id: &str, turn_no: u32, speaker: &str) -> SessionTurnV1 {
    SessionTurnV1 {
        session_id: session_id.to_string(),
        workspace_id: WORKSPACE.to_string(),
        speaker: speaker.to_string(),
        text: format!("{speaker}-{turn_no}"),
        turn_no,
        observed_at_ms: u64::from(turn_no),
    }
}

fn append_record(store: &Store, claim_id: &str, observed_at_ms: u64) -> TestResult {
    let coord = coordinate_for_claim(WORKSPACE, claim_id)?;
    let payload = claim_recorded(claim_id, observed_at_ms);
    let _ = store.apply_transition(&coord, record_claim(payload))?;
    Ok(())
}

fn append_supersede(
    store: &Store,
    old_claim_id: &str,
    new_claim_id: &str,
    reason: &str,
    observed_at_ms: u64,
) -> TestResult {
    let coord = coordinate_for_claim(WORKSPACE, old_claim_id)?;
    let payload = claim_superseded(old_claim_id, new_claim_id, reason, observed_at_ms);
    let _ = store.apply_transition(&coord, supersede_claim(payload))?;
    Ok(())
}

fn append_conflict(
    store: &Store,
    conflict_id: &str,
    claim_a: &str,
    claim_b: &str,
    observed_at_ms: u64,
) -> TestResult {
    let coord = coordinate_for_conflict(WORKSPACE, conflict_id)?;
    let payload = conflict_opened(conflict_id, claim_a, claim_b, observed_at_ms);
    let _ = store.apply_transition(&coord, open_conflict(payload))?;
    Ok(())
}

fn claim_by_id<'a>(claims: &'a [ClaimView], claim_id: &str) -> &'a ClaimView {
    claims
        .iter()
        .find(|claim| claim.card.claim_id == claim_id)
        .expect("claim is present")
}

fn populate_three_claim_workspace(store: &Store) -> TestResult {
    append_record(store, "claim_a", 1)?;
    append_record(store, "claim_b", 2)?;
    append_record(store, "claim_c", 3)?;
    append_supersede(store, "claim_a", "claim_b", "newer", 4)?;
    append_conflict(store, "conflict_a", "claim_b", "claim_c", 5)?;
    Ok(())
}

#[test]
fn fold_matches_incremental() -> TestResult {
    let whole_dir = TempDir::new()?;
    let whole = open_store(&whole_dir)?;
    append_record(&whole, "claim_a", 1)?;
    append_supersede(&whole, "claim_a", "claim_b", "newer", 2)?;
    let whole_card = whole
        .project::<ClaimCard>("claim:claim_a", &Freshness::Consistent)?
        .expect("whole projection has state");

    let incremental_dir = TempDir::new()?;
    let incremental = open_store(&incremental_dir)?;
    append_record(&incremental, "claim_a", 1)?;
    let after_first = incremental
        .project::<ClaimCard>("claim:claim_a", &Freshness::Consistent)?
        .expect("incremental projection has first state");
    assert_eq!(after_first.phase, 1);
    append_supersede(&incremental, "claim_a", "claim_b", "newer", 2)?;
    let incremental_card = incremental
        .project::<ClaimCard>("claim:claim_a", &Freshness::Consistent)?
        .expect("incremental projection has final state");

    assert_eq!(whole_card, incremental_card);
    Ok(())
}

#[test]
fn assembly_deterministic_across_reopen() -> TestResult {
    let dir = TempDir::new()?;
    let config = StoreConfig::new(dir.path()).with_sync_every_n_events(1);
    let store = Store::open(config.clone())?;
    populate_three_claim_workspace(&store)?;
    let mut cache = WorkspaceCache::default();
    let first = serde_json::to_vec(&assemble(&store, WORKSPACE, &mut cache)?)?;
    drop(store);

    let reopened = Store::open(config)?;
    let mut reopened_cache = WorkspaceCache::default();
    let second = serde_json::to_vec(&assemble(&reopened, WORKSPACE, &mut reopened_cache)?)?;

    assert_eq!(first, second);
    Ok(())
}

#[test]
fn cache_avoids_reprojection() -> TestResult {
    let dir = TempDir::new()?;
    let store = open_store(&dir)?;
    populate_three_claim_workspace(&store)?;
    let mut cache = WorkspaceCache::default();

    let _ = assemble(&store, WORKSPACE, &mut cache)?;
    let misses_after_first = cache.project_misses;
    let first_claim_generation = store
        .entity_generation("claim:claim_a")
        .expect("claim generation exists");
    let _ = assemble(&store, WORKSPACE, &mut cache)?;
    let second_claim_generation = store
        .entity_generation("claim:claim_a")
        .expect("claim generation exists");

    assert_eq!(cache.project_misses, misses_after_first);
    assert_eq!(first_claim_generation, second_claim_generation);
    Ok(())
}

#[test]
fn persisted_cache_uses_zero_delta_warm_view_and_advances_only_dirty_entity() -> TestResult {
    let dir = TempDir::new()?;
    let store = open_store(&dir)?;
    populate_three_claim_workspace(&store)?;
    let mut cache = WorkspaceCache::default();
    let initial = assemble(&store, WORKSPACE, &mut cache)?;
    cache.save(dir.path(), WORKSPACE)?;

    let mut warm = WorkspaceCache::load(dir.path(), WORKSPACE);
    let reopened = assemble(&store, WORKSPACE, &mut warm)?;
    assert_eq!(reopened, initial);
    assert_eq!(warm.freshness(), ProjectionFreshness::Fresh);
    // Sidecar v2 persists folded cards only: the warm path proves itself by
    // doing zero discovery, zero folds, and zero store projections — the one
    // derived view rebuild runs from persisted card state alone.
    assert_eq!(warm.counters.discovery_entries_paged, 0);
    assert_eq!(warm.counters.folded_events, 0);
    assert_eq!(warm.counters.project_calls, 0);
    assert_eq!(warm.counters.view_rebuilds, 1);

    append_record(&store, "claim_d", 6)?;
    let advanced = assemble(&store, WORKSPACE, &mut warm)?;
    assert_eq!(advanced.claims.len(), initial.claims.len() + 1);
    assert_eq!(warm.counters.discovery_entries_paged, 1);
    assert_eq!(warm.counters.folded_events, 1);
    assert_eq!(warm.counters.project_calls, 0);
    Ok(())
}

#[test]
fn missing_or_swapped_sidecar_rebuilds_to_source_truth() -> TestResult {
    let first_dir = TempDir::new()?;
    let first_store = open_store(&first_dir)?;
    populate_three_claim_workspace(&first_store)?;
    let mut first_cache = WorkspaceCache::default();
    let _ = assemble(&first_store, WORKSPACE, &mut first_cache)?;
    first_cache.save(first_dir.path(), WORKSPACE)?;

    let sidecar = first_dir.path().join(".texo/cache/workspace-view/demo.bin");
    std::fs::remove_file(&sidecar)?;
    let mut missing = WorkspaceCache::load(first_dir.path(), WORKSPACE);
    let rebuilt = assemble(&first_store, WORKSPACE, &mut missing)?;
    let mut fresh = WorkspaceCache::default();
    assert_eq!(rebuilt, assemble(&first_store, WORKSPACE, &mut fresh)?);

    first_cache.save(first_dir.path(), WORKSPACE)?;
    std::fs::write(&sidecar, b"not-msgpack")?;
    let mut corrupt = WorkspaceCache::load(first_dir.path(), WORKSPACE);
    assert_eq!(corrupt.freshness(), ProjectionFreshness::Invalid);
    assert_eq!(assemble(&first_store, WORKSPACE, &mut corrupt)?, rebuilt);

    first_cache.save(first_dir.path(), WORKSPACE)?;
    let mut swapped = WorkspaceCache::load(first_dir.path(), WORKSPACE);
    let second_dir = TempDir::new()?;
    let second_store = open_store(&second_dir)?;
    append_record(&second_store, "claim_other", 1)?;
    let swapped_view = assemble(&second_store, WORKSPACE, &mut swapped)?;
    let mut second_fresh = WorkspaceCache::default();
    assert_eq!(
        swapped_view,
        assemble(&second_store, WORKSPACE, &mut second_fresh)?
    );
    assert_eq!(swapped_view.claims.len(), 1);
    assert_eq!(swapped_view.claims[0].card.claim_id, "claim_other");
    Ok(())
}

#[test]
fn conflict_marks_both_claims() -> TestResult {
    let dir = TempDir::new()?;
    let store = open_store(&dir)?;
    append_record(&store, "claim_a", 1)?;
    append_record(&store, "claim_b", 2)?;
    append_conflict(&store, "conflict_a", "claim_a", "claim_b", 3)?;

    let mut cache = WorkspaceCache::default();
    let view = assemble(&store, WORKSPACE, &mut cache)?;

    assert_eq!(
        claim_by_id(&view.claims, "claim_a").status,
        ClaimStatus::Conflicting
    );
    assert_eq!(
        claim_by_id(&view.claims, "claim_b").status,
        ClaimStatus::Conflicting
    );
    Ok(())
}

#[test]
fn superseded_beats_conflicting() -> TestResult {
    let dir = TempDir::new()?;
    let store = open_store(&dir)?;
    append_record(&store, "claim_a", 1)?;
    append_record(&store, "claim_b", 2)?;
    append_record(&store, "claim_c", 3)?;
    append_supersede(&store, "claim_a", "claim_b", "newer", 4)?;
    append_conflict(&store, "conflict_a", "claim_a", "claim_c", 5)?;

    let mut cache = WorkspaceCache::default();
    let view = assemble(&store, WORKSPACE, &mut cache)?;

    assert_eq!(
        claim_by_id(&view.claims, "claim_a").status,
        ClaimStatus::Superseded
    );
    assert_eq!(
        claim_by_id(&view.claims, "claim_c").status,
        ClaimStatus::Conflicting
    );
    Ok(())
}

#[test]
fn anomaly_on_duplicate_supersede() -> TestResult {
    let dir = TempDir::new()?;
    let store = open_store(&dir)?;
    append_record(&store, "claim_a", 1)?;
    append_supersede(&store, "claim_a", "claim_b", "first", 2)?;
    append_supersede(&store, "claim_a", "claim_c", "second", 3)?;

    let card = store
        .project::<ClaimCard>("claim:claim_a", &Freshness::Consistent)?
        .expect("claim projection has state");

    assert_eq!(card.superseded_by, Some("claim_b".to_string()));
    assert_eq!(card.superseded_reason, "first");
    assert!(card
        .anomalies
        .iter()
        .any(|item| item == "duplicate-supersede"));
    Ok(())
}

#[test]
fn session_turns_project() -> TestResult {
    let dir = TempDir::new()?;
    let store = open_store(&dir)?;
    let session_id = "session_a";
    let lane = session_lane(session_id);
    let coord = coordinate_for_session(WORKSPACE, session_id)?;
    let entity = entity_for_session(session_id);
    let scope = scope_for_workspace(WORKSPACE);

    let fence = store.begin_visibility_fence()?;
    let mut outbox = fence.outbox();
    outbox.stage_typed_with_options(
        coord.clone(),
        &session_turn(session_id, 2, "assistant"),
        AppendOptions::new().with_position_hint(AppendPositionHint::branch_root(lane, 0)),
    )?;
    outbox.stage_typed_with_options(
        coord.clone(),
        &session_turn(session_id, 1, "user"),
        AppendOptions::new().with_position_hint(AppendPositionHint::new(lane, 1)),
    )?;
    outbox.stage_typed_with_options(
        coord,
        &session_turn(session_id, 3, "user"),
        AppendOptions::new().with_position_hint(AppendPositionHint::new(lane, 2)),
    )?;
    let ticket = outbox.submit_flush()?;

    assert!(store.query(&Region::scope(&scope)).is_empty());

    fence.commit()?;
    let receipts = ticket.receiver().recv_timeout(Duration::from_secs(2))??;
    assert_eq!(receipts.len(), 3);

    let log = store
        .project::<SessionLog>(&entity, &Freshness::Consistent)?
        .expect("session projection has state");
    let turn_numbers = log
        .turns
        .iter()
        .map(|turn| turn.turn_no)
        .collect::<Vec<_>>();
    assert_eq!(turn_numbers, vec![1, 2, 3]);
    Ok(())
}

/// Folded card states must be indistinguishable from full store replays, on
/// both the cold-rebuild path and the warm incremental-delta path.
#[test]
fn folded_cards_match_store_replay() -> TestResult {
    let dir = TempDir::new()?;
    let store = open_store(&dir)?;
    populate_three_claim_workspace(&store)?;
    append_conflict(&store, "conflict_1", "claim_a", "claim_b", 30)?;

    // Cold path: fold everything from sequence zero.
    let mut cache = WorkspaceCache::default();
    let cold = assemble(&store, WORKSPACE, &mut cache)?;

    // Warm path: new events fold as a delta onto the cached states.
    append_record(&store, "claim_d", 40)?;
    append_supersede(&store, "claim_d", "claim_a", "delta supersede", 41)?;
    let warm = assemble(&store, WORKSPACE, &mut cache)?;
    assert!(warm.claims.len() > cold.claims.len());
    assert!(cache.counters.folded_events > 0, "delta path must fold");
    assert_eq!(
        cache.counters.project_calls, 0,
        "steady state must not hit the store projection repair path"
    );

    // Ground truth: every folded card equals its full store replay.
    for view in &warm.claims {
        let entity = format!("claim:{}", view.card.claim_id);
        let replayed = store
            .project::<ClaimCard>(&entity, &Freshness::Consistent)?
            .expect("claim entity must replay");
        assert_eq!(view.card, replayed, "fold diverged for {entity}");
    }
    for conflict in &warm.conflicts {
        let entity = format!("conflict:{}", conflict.conflict_id);
        let replayed = store
            .project::<texo::claims::conflict::ConflictCard>(&entity, &Freshness::Consistent)?
            .expect("conflict entity must replay");
        assert_eq!(conflict, &replayed, "fold diverged for {entity}");
    }
    Ok(())
}
