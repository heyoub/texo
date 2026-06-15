//! PROVES: INV-REPLAY-ERRORS (F3/F4 decode path) — an event whose kind matches
//! none of texo's five known kinds must surface as `DecodeError::UnsupportedKind`
//! rather than being silently skipped during workspace replay (SPEC.md:73 — no
//! silent partial state).

mod support;

use batpak::prelude::*;
use serde::{Deserialize, Serialize};
use support::{setup_demo_journal, temp_workspace};
use texo_core::open_journal;

/// A payload in texo's category (0xE) but with a `type_id` that does NOT match
/// any of the five registered texo kinds (1..=5). Decoding it must fail loudly.
#[derive(Debug, Clone, Serialize, Deserialize, EventPayload)]
#[batpak(category = 0xE, type_id = 0x099)]
struct UnknownTexoEvent {
    note: String,
}

#[test]
fn replay_errors_on_unknown_event_kind() {
    let dir = temp_workspace();
    let journal = setup_demo_journal(dir.path());
    let workspace = journal.config().workspace().expect("workspace");

    // Append a real, committed event with an unknown kind directly into the
    // workspace scope via the BatPak store. It lands in the same region that
    // `load_workspace_events` scans, so replay must encounter it.
    let coord = Coordinate::new("claim:unknownkind", workspace.scope()).expect("coordinate");
    let _receipt = journal
        .handle()
        .store()
        .append_typed(
            &coord,
            &UnknownTexoEvent {
                note: "injected".to_string(),
            },
        )
        .expect("append unknown-kind event");

    journal.close().expect("close");

    // Reopen and attempt to load/replay the workspace events. The unknown kind
    // must propagate as a decode error, not be silently dropped.
    let journal = open_journal(dir.path()).expect("reopen");
    let result = journal.handle().load_events(&workspace);
    journal.close().expect("close");

    let err = result.expect_err("loading an unknown kind must error, not skip it");
    let msg = err.to_string();
    assert!(
        msg.contains("unsupported event kind"),
        "expected DecodeError::UnsupportedKind, got: {msg}"
    );
}
