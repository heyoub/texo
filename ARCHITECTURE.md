# Architecture

Supersedes: see [ADR-003](ADR-003-single-crate-rebuild.md).

```txt
markdown / Git snapshot / code index / session transcript
  -> extractor: heuristic or LLM record-once cache
  -> bounded evidence occurrence + analysis quality
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

- `events`: ten typed payload kinds, transition evidence, coordinate builders,
  stable domain IDs, and durable relation judgment/deferral facts.
- `claims`: `ClaimCard`, `ConflictCard`, `SourceCard`, `CompileLog`,
  `WorkspaceCard`, `SessionLog`, derived statuses, and `WorkspaceView`.
- `extract`: markdown parser, heuristic extractor, LLM extractor, faithfulness
  gate, and content-addressed record-once caches.
- `gateway` and `semantics`: one provider-neutral configuration path for the
  closed `Embed`, `Propose`, `Relate`, and `Chat` roles, plus OpenAI-compatible
  transport adapters.
- `ops`: syncbat operations, thread-local `OpEnv`, effect backend routing, and
  operation catalog.
- `host`: workspace store opening, capability grants, invocation seam, and the
  `texo-canonical-v1` interface fingerprint.
- `surfaces`: CLI, sync HTTP/1.1 server/client, SSE, OpenAI-compatible edge,
  bootstrap, and MCP stdio.
- `agent_catalog`, `install`, and `hooks`: the five-tool progressive-disclosure
  catalog, structural client adapters, managed guidance, and fixed advisory
  read hooks.
- `doctor` and `backup`: composed operator diagnostics and an evidence-backed
  journal/config portability boundary.
- `knowledge`: snapshot tokens, Git object identities, evidence occurrences,
  temporal partial-order results, coverage gaps, and code-analysis quality.
- `code_index`: bounded SCIP import, a pinned Rust tree-sitter tags analyzer,
  the universal lexical floor, and authenticated disposable normalized
  artifacts.
- `claims::evidence`: a replay-only projection that joins exact occurrence
  events to claim links at a requested frontier; missing disposable indexes do
  not alter this view.

The evidence, structural, and belief planes and their replay boundary are
frozen in [ADR-004](ADR-004-snapshot-evidence-temporal-model.md).

## Operation Catalog

The catalog is content-addressed by `texo host fingerprint`. It currently
contains 25 operations:

`texo.workspace.init`, `texo.workspace.status`, `texo.ingest.run`,
`texo.knowledge.index`, `texo.code.index.build`, `texo.knowledge.triangulate`,
`texo.claims.list`, `texo.claims.search`, `texo.knowledge.search`,
`texo.claim.explain`, `texo.claim.supersede`, `texo.staleness.check`,
`texo.context.agent`, `texo.compile.run`, `texo.conflicts.list`,
`texo.conflicts.commit`, `texo.conflict.resolve`, `texo.verify.run`,
`texo.relate.run`, `texo.stats.read`, `texo.host.fingerprint`, `texo.agent.chat`,
`texo.agent.memory`, `texo.agent.session.end`, and `texo.session.export`.

## Transports

- CLI calls `TexoHost::invoke_json` and renders human text or pretty JSON.
- HTTP is hand-rolled sync HTTP/1.1: 8 KiB request head cap, 1 MiB POST body
  cap, dual permit pools, contained worker panics, and static UI fallback.
- SSE subscribes to BatPak store notifications and emits strict JSON signal
  frames with `id:` equal to global sequence.
- MCP is line-delimited JSON-RPC 2.0 over locked stdin/stdout and exposes five
  read-only, grant-described tools backed by the same typed operation catalog.
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
