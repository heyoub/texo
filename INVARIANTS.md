# Invariants

Supersedes: see [ADR-003](ADR-003-single-crate-rebuild.md).

| ID | Invariant | Test Anchor |
|---|---|---|
| INV-REPLAY-DETERMINISTIC | Replaying a reopened store produces byte-identical workspace views and stable goldens. | `tests/projection_laws.rs`, `tests/idempotent_replay.rs`, `tests/replay_truth.rs` |
| INV-RECEIPT-VERIFIED-L1 | Every domain append receipt verifies against the BatPak store before surfacing. | `tests/spike_family.rs`, `tests/ingest_receipts.rs`, `tests/ops_kit.rs` |
| INV-RECEIPT-VERIFIED-L2 | syncbat operation receipt envelopes are journaled at the op receipt coordinate. | `tests/ops_kit.rs` |
| INV-JOURNAL-VALID | `texo.verify.run` decodes every workspace event, verifies the chain, and reports unsupported or foreign kinds as findings. | `tests/decode_unsupported_kind.rs`, `tests/error_paths_journal.rs` |
| INV-TRANSITION-EVIDENCE | Claim/conflict phase changes use legal typestate edges and deterministic transition records. | `tests/compile_fail.rs`, `tests/projection_laws.rs`, `src/events/machines.rs` |
| INV-OPS-FAIL-CLOSED | Undeclared effects, unknown payload kinds, missing receipt sinks, and missing capabilities deny before state changes. | `tests/ops_kit.rs` |
| INV-CONFLICT-SEMANTICS | Conflicts are contradictory current claims; superseded claims do not beat the superseded status. | `tests/conflicts_courtroom.rs`, `tests/projection_laws.rs` |
| INV-SESSION-LANES | Session turns are crash-durable in non-zero lanes and hidden from lane-0 memory until session-end ingest. | `tests/session_lanes.rs`, `tests/http_server.rs` |
| INV-STALE-EXACT-LINE | Staleness diagnostics point at exact source lines. | `tests/staleness_courtroom.rs`, `tests/golden_staleness.rs` |
| INV-AGENT-CONTEXT-FRONTIER | Agent/MCP context includes replay frontier, provenance, current claims, stale claims, and conflicts. | `tests/agent_context.rs`, `tests/mcp_stdio.rs`, `tests/golden_agent_context.rs` |
| INV-AGENT-CATALOG-BOUNDED | MCP exposes exactly five read-only progressive-disclosure tools; search and hook inputs are bounded. | `tests/agent_catalog.rs`, `tests/mcp_stdio.rs`, `tests/advisory_hooks.rs` |
| INV-INSTALL-OWNERSHIP | Install is idempotent and structurally merges only Texo-owned entries; uninstall preserves user config and journal data. | `tests/appliance_install.rs`, `src/install.rs` |
| INV-BACKUP-AUTHORITY | Backups contain journal snapshot evidence plus config, exclude derived caches, verify offline without mutation, and fail closed on tampering. | `tests/backup.rs`, `src/backup.rs` |
