# ADR-003: single-crate rebuild

Date: 2026-07-06

## Status

Accepted.

## Context

texo started as six crates plus a VS Code shell: core, CLI, MCP, semantics,
extractor, and agent. The runtime also carried three async transports and a
large web stack. Meanwhile the BatPak family grew the primitives texo needed:
syncbat operations, host composition surfaces, lanes, transition evidence, and
receipt-oriented effect checks.

The old architecture worked, but it had too many integration seams for a local
claim-chain memory tool. The rebuild deliberately kept the old prose and ADRs
as drift corpus while replacing the runtime with a single sync-first binary.

## Decision

texo is now one crate and one binary:

- Single package `texo` with `src/lib.rs` and `src/bin/texo.rs`.
- sync-first implementation, no tokio/reqwest/axum/tower/rmcp/schemars/anyhow.
- syncbat Core operations are the only handler surface.
- `TexoEffectBackend` is the append chokepoint: typed decode, coordinate/lane
  routing, transition application, and inline receipt verification.
- Event schema v2 is a clean break: eight payload kinds in category `0xE`, no
  serde defaults, transition evidence on claim/conflict changes.
- Per-entity BatPak projections feed deterministic `WorkspaceView` assembly.
- Session lanes make the journal the session: turns are durable hidden lane
  events; session-end ingest records lane-0 source and claim events.
- HTTP server, SSE, MCP stdio, and OpenAI-compatible HTTP client are
  hand-rolled synchronous transports.
- The BatPak lint bar is the repo quality bar.

Hostbat 0.9.0 has a HostBuilder gap, so WO-4 introduced a temporary
`texo-canonical-v1` interface fingerprint over the declared operation surface.
Roadmap tracks replacing it with hostbat manifests after the BatPak 0.10 bump
(freebatteryfactory/batpak#166, fixed in #169/0.10.0).

## Drift Methodology

Old ADRs and docs stay standing as prose inputs, not as current architecture.
`just drift` copies the repo's markdown into a temporary texo workspace,
ingests it, optionally runs semantic relate when a key is present, and prints
current/stale/conflict claims. That demonstrates texo-on-texo supersession
instead of silently rewriting history.

## Consequences

- Dependency surface drops by roughly two hundred crates by removing the async
  web stack and old split-crate duplication.
- One release artifact deploys everything: CLI, extractor, server, MCP, and
  session export.
- Claim-id identity is preserved across the schema-v2 rebuild; QA-3 verified
  the old demo claim IDs did not churn.
- Stores are schema-breaking by design. v2 is a clean break with deterministic
  replay and explicit transition evidence.
- The VS Code extension remains a thin CLI caller.
- Future work is clearer: batpak 0.10 hostbat remount, MemFs/SimFs test stores,
  SSE replay endpoints, and replay-to-WASM.
