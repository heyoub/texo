# Architecture

```txt
markdown sources
   ↓ ingest — heuristic extract (default)  OR  AST → texo-extract (LLM) → faithfulness gate
BatPak journal (.texo/store)
   ↓ relate (optional): embed → cluster (connected components) → within-cluster prefilter → LLM relation-judge → supersede/conflict events
   ↓ append typed events + verify receipts
replay (query_entries_after)
   ↓ ClaimState projection (errors propagate)
agent JSON / staleness / compile / MCP / VS Code CLI shell
```

The semantic path is opt-in per workspace (`[semantics]`) and stays **outside**
`texo-core`'s HTTP-free boundary: `texo-extract` (binary, via the `extract_via_cmd`
seam) and `texo-semantics` (hosted OpenAI-compatible backends — OpenRouter by
default, any compatible host via `OPENROUTER_BASE_URL` — plus local ONNX) hold
all model/HTTP code;
the model runs once at ingest (record-once) and only its journaled events feed
replay. See [`ADR-001`](ADR-001-semantic-pipeline.md).

texo is a single-writer local claim-chain. Sequences are per-store. No global order or consensus is claimed.

## Typestate split

- **`Journal<Open | Closed>`** — workspace config + root path; `Open` holds a `StoreHandle`, `Closed` does not.
- **`StoreHandle { store: Store<Open> }`** — BatPak I/O handle; `close()` runs BatPak store lifecycle.
- Domain replay and projection live outside the journal module; only append/decode/query touch BatPak APIs.

## Verify surfaces

- **`verify_projection`** — replayed `ClaimState` invariants (supersession consistency).
- **`verify_journal_receipts`** — BatPak wire receipt verification over journaled texo events.
- CLI `texo verify` runs both and reports `{ projection_ok, journal_ok, errors }`.

## Conflict semantics (v0)

`detect_conflicts` emits open conflicts only for **contradictory current claims** (replacement keywords, predicate mismatch, deploy/release subject hints). Supersession edges are not reported as conflicts.

## Extractor composition (v1)

```txt
plan_sources_for_config
  ├─ extractor_cmd set? → extract_via_cmd (NDJSON subprocess)
  └─ else → extract_claims (heuristic-v1) or injected ExtractClaimsFn in tests
```

No `Extractor` trait — plain fn pointer at the ingest boundary.

## Multi-workspace

[`TexoRootConfig`](crates/texo-core/src/config.rs) maps workspace ids to store paths. `open_journal_with(root, Some("staging"))` opens the correct BatPak store for that scope.

See [`SPEC.md`](SPEC.md) and [`INVARIANTS.md`](INVARIANTS.md).
