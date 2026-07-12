# Upstream & reconciliation ledger

Every mechanism this campaign touched, classified per the shim/upstream rule.
Filed issues carry neutral reproducers only — no texo vocabulary upstream.

## Filed against batpak (2026-07-12)

| Issue | Finding | Evidence |
|---|---|---|
| [#203](https://github.com/freebatteryfactory/batpak/issues/203) | Per-entity `Store::project` builds an O(scope) replay plan per call; incremental flag doesn't remove the scan | texo's ingest went quadratic through it (17s @25k → 92s @55k → ~90min @230k, pure CPU); fixed downstream by folding paged events via `EventSourced::apply_event` (O(delta)) |
| [#204](https://github.com/freebatteryfactory/batpak/issues/204) | `apply_transition` cannot carry `AppendOptions` (durable idempotency) | Workaround `into_payload` + `append_typed_with_options` pinned byte-identical by test |
| [#205](https://github.com/freebatteryfactory/batpak/issues/205) | No public store identity for anchoring derived caches | texo invented a frontier+event-id anchor check for its projection sidecar |
| [#206](https://github.com/freebatteryfactory/batpak/issues/206) | No region-scoped frontier accessor | Low priority; derived by paging today |

## Filed after duplicate-check against existing open issues (2026-07-12)

- [#217](https://github.com/freebatteryfactory/batpak/issues/217) —
  active-segment point reads serialize on one `Mutex<FdCache>` (sealed reads
  are concurrent; the tail is the fan-out ceiling; futex evidence attached).
- Duplicate-check outcome for the earlier filings: not dupes, cross-linked —
  #203 ↔ #199 (MODEL-MATERIALIZED-PROJECTION-V1 owns the designed bulk-replay
  answer; #203 stays as the measured defect record) and #205 ↔ #195
  (GAUNT-AUTHORITY-MANIFEST's generation boundary subsumes the store-identity
  ask if exposed publicly; #205 can close into it).

## Texo domain (tracked as texo issues, not upstream — batpak stays domain-free)

- [texo#1](https://github.com/heyoub/texo/issues/1) — pair budget / top-K
  candidate guard (44k pairs @0.65 / 10.5k @0.8 measured).
- [texo#2](https://github.com/heyoub/texo/issues/2) — parallel judge fan-out
  with deterministic settlement order (+ extraction fan-out).
- [texo#3](https://github.com/heyoub/texo/issues/3) — `relate --rejudge-pair`
  operator lever for suspect cached verdicts.
- [texo#4](https://github.com/heyoub/texo/issues/4) — cache tmp-file pid
  suffix (cross-process torn-rename hazard).

## Lessons for MemBat (reconciliation items; no MemBat code changed)

1. **Content-derived idempotency keys beat process-nonce attempt ids** for
   resumable settlement: a different process resuming the same logical work
   still deduplicates (MemBat's `hash(identity, nonce, request)` only
   dedupes within one attempt).
2. **Holdback for derived decisions**: absence of a verdict must block
   dependent supersessions/conflicts (tainted-claim rule), not just be
   reported.
3. **First-valid-judgment-wins at the projection**, later contrary judgments
   append as history but drive no derived events.
4. **Projection sidecars must persist folded state only** — persisting the
   derived view too doubled every text and made the sidecar outweigh the
   journal (387MB vs 267MB).
5. **Fold paged events instead of per-entity `project()` calls** (see #203) —
   MemBat's history projection composes the same substrate surface.
6. **Per-read wall-deadline enforcement in blocking HTTP clients**:
   `SO_RCVTIMEO` resets per delivered byte; a trickling provider turns one
   bounded call into hours. MemBat's client shares this ancestry — check it.
7. **Bench-science**: oracle templates cross-cluster if same-shaped
   (embedding similarity ignores the name slot); noise prefixes percolate
   clusters; measurement runs must not overlap.
