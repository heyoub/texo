# Architecture

Supersedes: see [ADR-003](ADR-003-single-crate-rebuild.md).

```txt
markdown / session transcript
  -> extractor: heuristic or LLM record-once cache
  -> syncbat op
  -> TexoEffectBackend
  -> BatPak Store<Open>
  -> per-entity EventSourced projections
  -> CLI / HTTP / SSE / MCP / static compile
```

texo is a single-crate, single-binary, sync-first application. BatPak owns the
append-only journal and receipts. syncbat owns operation declaration and effect
checks. texo owns domain schema, projections, and user surfaces.

## Modules

- `events`: eight v2 payloads, transition evidence, coordinate builders, and
  stable domain IDs.
- `claims`: `ClaimCard`, `ConflictCard`, `SourceCard`, `CompileLog`,
  `WorkspaceCard`, `SessionLog`, derived statuses, and `WorkspaceView`.
- `extract`: markdown parser, heuristic extractor, LLM extractor, faithfulness
  gate, and content-addressed record-once caches.
- `semantics`: OpenAI-compatible embedding, relation, NLI, rerank, proposer,
  and chat builders behind the `openrouter` feature.
- `ops`: syncbat operations, thread-local `OpEnv`, effect backend routing, and
  operation catalog.
- `host`: workspace store opening, capability grants, invocation seam, and the
  `texo-canonical-v1` interface fingerprint.
- `surfaces`: CLI, sync HTTP/1.1 server/client, SSE, OpenAI-compatible edge,
  bootstrap, and MCP stdio.

## Operation Catalog

The catalog is content-addressed by `texo host fingerprint`. It currently
contains 18 operations:

`texo.workspace.init`, `texo.ingest.run`, `texo.claims.list`,
`texo.claim.explain`, `texo.claim.supersede`, `texo.staleness.check`,
`texo.context.agent`, `texo.compile.run`, `texo.conflicts.list`,
`texo.conflicts.commit`, `texo.conflict.resolve`, `texo.verify.run`,
`texo.relate.run`, `texo.host.fingerprint`, `texo.agent.chat`,
`texo.agent.memory`, `texo.agent.session.end`, and `texo.session.export`.

## Transports

- CLI calls `TexoHost::invoke_json` and renders human text or pretty JSON.
- HTTP is hand-rolled sync HTTP/1.1: 8 KiB request head cap, 1 MiB POST body
  cap, dual permit pools, contained worker panics, and static UI fallback.
- SSE subscribes to BatPak store notifications and emits strict JSON signal
  frames with `id:` equal to global sequence.
- MCP is line-delimited JSON-RPC 2.0 over locked stdin/stdout and exposes four
  read-only tools.
- The model client is hand-rolled sync HTTP/1.1 with rustls/ring and
  webpki-roots behind the `openrouter` feature.

## Lanes

Session turns are appended to a deterministic non-zero lane from
`session_lane(session_id)`. Lane events survive crash and reopen but remain
hidden from default lane projections. Session end renders user turns into a
markdown transcript, ingests that transcript, and runs relate when the model
capability is present.

## Determinism

Workspace assembly sorts entities and uses BatPak entity generations with
`project_if_changed`. Replay does not depend on hash-map iteration order.
Receipts are verified inline at append, and `texo verify` sweeps journal decode,
chain verification, projection anomalies, and transition reports.
