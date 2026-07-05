# WO-0 Deferred Items

- `crates/texo-core/src/extract/mod.rs` — `ExtractClaimsFn` and extraction plumbing — references old claim event payloads and ingest/journal schema; defer until schema v2.
- `crates/texo-core/src/replay/state.rs` — canonical `ClaimView` projection — derives from old replay state and claim lifecycle schema; defer until schema v2. WO-0 keeps only the minimal `semantics::pipeline` bridge needed by the pure cluster-first tests.
- `crates/texo-core/src/types/receipt.rs` — canonical receipt view surface — references old replay/journal receipt verification links; defer until schema v2. WO-0 keeps only the sequence bridge needed by the pure cluster-first tests.
- `crates/texo-core/src/types/status.rs` and `crates/texo-core/src/state/conflict_lifecycle.rs` — canonical claim/conflict lifecycle status and conflict entry surfaces — tied to old replay lifecycle state; defer until schema v2.
