# Architecture

```txt
markdown sources
   ↓ ingest (heuristic extract)
BatPak journal (.texo/store)
   ↓ append typed events + verify receipts
replay (query_entries_after)
   ↓ ClaimState projection (errors propagate)
agent JSON / staleness / compile / MCP / VS Code CLI shell
```

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

See [`INVARIANTS.md`](INVARIANTS.md) and [`SPEC.md`](SPEC.md).
