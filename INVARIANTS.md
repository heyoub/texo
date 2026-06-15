# Invariants

| ID | Invariant | Test |
|---|---|---|
| INV-REPLAY-CURRENT-UNIQUE | Superseded claims never appear as current | `tests/replay_truth.rs` |
| INV-REPLAY-SUPERSESSION | Supersession events flip status deterministically | `tests/replay_truth.rs` |
| INV-REPLAY-DETERMINISTIC | Close/reopen replay yields identical state | `tests/idempotent_replay.rs` |
| INV-REPLAY-ERRORS | Replay propagates illegal transition failures | `src/replay/reducer.rs`, `tests/replay_truth.rs` |
| INV-RECEIPT-VERIFIED | Committed appends verify against the BatPak store | `tests/ingest_receipts.rs`, `journal/receipt.rs` |
| INV-COMPILE-JOURNALED | Compile appends `OnboardingCompiled` to the journal | `tests/compile_journaled.rs` |
| INV-CONFLICT-SEMANTICS | Conflicts are contradictory current claims, not supersession edges | `tests/conflicts_courtroom.rs`, `src/conflicts/detect.rs` |
| INV-STALE-EXACT-LINE | Stale prose flagged at exact source line | `tests/staleness_courtroom.rs` |
| INV-AGENT-CONTEXT-FRONTIER | Agent JSON includes frontier + provenance | `tests/agent_context.rs`, `tests/golden_agent_context.rs` |
| THESIS-STALE-ONBOARDING | Demo stale onboarding exposes supersession | `tests/thesis_meta.rs` |
