# BatPak Scale-Out Composition

This document freezes how Texo uses the BatPak 0.10 family. The physical-store
lock is an ordering and ownership guarantee for one materialization. It is not
a global throughput ceiling. Texo scales horizontally by coordinate and by
CQRS: authoritative events enter a canonical journal, then any number of
independent read-model stores materialize that ordered history.

The useful Redux analogy is exact. A BatPak coordinate is a normalized state
key, events are actions, the canonical fold is the reducer, and replica stores
are independently memoized selectors. Each physical store has one owner;
separate stores open and serve concurrently.

## Product topology

`texo install --client all` creates three local imported read models and routes
Codex, Claude Code, and Cursor to distinct journals. MCP startup bootstraps or
resumes its selected replica from a durable cursor before opening the reader.
Lease contention during simultaneous startup has a bounded typed retry and
does not weaken BatPak's store contract.

Canonical journals admit persist, emit, and control operations. Replica hosts
reject those effects before dispatch. Snapshot tokens bind workspace,
journal, local frontier, anchor event, and source snapshot, so a token from one
materialization cannot be replayed against another.

Two replica modes stay intentionally distinct:

- `exact_fork` is a point-in-time structural clone preserving event identity.
  It records BatPak fork evidence and never silently becomes authoritative.
- `imported_read_model` materializes source events under deterministic
  destination-local identities, records batch evidence atomically, and resumes
  from a source cursor and anchor. It is the normal local and hosted read path.

Remote circuits use netbat framing over a private concrete IP only. Requests
carry a keyed BLAKE3 authentication tag over the complete request rather than
transmitting the secret. Responses are bounded before decoding. Restart
continues from durable evidence; a changed anchor or journal identity fails
closed.

## Composition ledger

| Mechanism | Classification | Implementation |
|---|---|---|
| Operation catalog and dispatch | `USE_EXISTING_BATPAK` | hostbat owns the sealed module, effects, receipts, and interface fingerprint. |
| Domain journal and projections | `USE_EXISTING_BATPAK` | BatPak events, coordinates, typed transitions, append options, replay, verification, and subscriptions. |
| Operation status | `USE_EXISTING_BATPAK` | syncbat status sinks and execution evidence; Texo does not create a second lifecycle engine. |
| Exact replica | `USE_EXISTING_BATPAK` | BatPak snapshot/fork evidence plus reopen verification. |
| Imported replica batches | `RECOMPOSE_BATPAK` | BatPak batch append, deterministic import identity, causation, receipts, and a Texo-domain replica ledger. |
| Backup proof | `USE_EXISTING_BATPAK` | BatPak canonical backup envelope and restore-proof report are embedded in Texo's operator manifest. |
| Hosted transport | `RECOMPOSE_BATPAK` | netbat framing and limits with a small synchronous client/server adapter. |
| Extractor confinement | `RECOMPOSE_BATPAK` | bvisor Linux policy and evidence run in an isolated companion process. There is no raw-command fallback. |
| Relation settlement, holdback, Git/code evidence | `TEXO_DOMAIN` | These are claim-authority semantics and do not belong in the neutral substrate. |
| Durable import dedup after reopen | `LOCAL_NEUTRAL_SHIM` | `src/compat/batpak.rs`; delete when BatPak issue #227 ships. |
| Bounded netbat client/decoder | `LOCAL_NEUTRAL_SHIM` | `src/compat/netbat.rs`; delete when issue #228 ships. |
| Isolated bvisor companion | `LOCAL_NEUTRAL_SHIM` | `src/compat/bvisor.rs` and `texo-bvisor-extractor`; delete after issues #232 and #233. |
| Rebuilding BatPak's event store, graph database, broker, or async runtime | `REJECT` | Duplicates proven family mechanisms or violates the sync-first single-crate contract. |

## Upstream replacement queue

Every neutral gap has a public reproducer and a deletion seam:

- [#227](https://github.com/freebatteryfactory/batpak/issues/227): imported
  event deduplication must survive destination reopen.
- [#228](https://github.com/freebatteryfactory/batpak/issues/228): bounded
  netbat response decoding and a blocking client.
- [#229](https://github.com/freebatteryfactory/batpak/issues/229): syncbat
  atomic typed batch-append effect capability.
- [#230](https://github.com/freebatteryfactory/batpak/issues/230): paged
  read-walk evidence without full matched-set materialization.
- [#231](https://github.com/freebatteryfactory/batpak/issues/231): bvisor
  content-addressed captured streams for host consumers.
- [#232](https://github.com/freebatteryfactory/batpak/issues/232): bvisor
  self-hosted launcher dispatch for single-binary applications.
- [#233](https://github.com/freebatteryfactory/batpak/issues/233): collision-free
  event-kind allocation for downstream family composition.

The bvisor 0.10 registry collision is why the helper is a separate process,
not a weakened registry check. Linux deployment ships `texo`,
`texo-bvisor-extractor`, and `bvisor-linux-launcher` together. The helper is
content- and path-bounded, uses a private staging directory, denies network,
and publishes output only after bvisor completion evidence. When #232/#233
land, the companion is replaced without changing the Texo extractor contract.

## Deliberate remaining boundaries

Texo does not invent consensus, shared mutation, or replica promotion. A
canonical journal is authority for its coordinate space. Larger deployments
partition coordinate spaces across canonical stores and fan read models from
each. Replica lag is explicit; authority is never inferred from availability.

The current imported circuit pages source events then atomically appends the
destination batch and ledger evidence. It does not journal one source event at
a time. Handler-level domain batches still await syncbat #229 rather than a
second local transaction abstraction. Read-only verification keeps BatPak's
chain proof and avoids a hand-rolled verifier while #230 tracks the neutral
paged evidence improvement.
