# texo

Supersedes: see [ADR-003](ADR-003-single-crate-rebuild.md).

texo is claim-chain memory for teams and agents. It ingests markdown, records
typed claims in a BatPak journal, supersedes stale claims with receipts, and
replays deterministic current context through CLI, HTTP, SSE, MCP, and static
compile surfaces.

Git tracks code diffs. texo tracks claim diffs.

See [INTELLIGENCE.md](INTELLIGENCE.md) for the agent-first Git/code workflow,
coverage contract, limitations, and independently reproducible proof.

## Quickstart

```sh
texo install --workspace demo
texo ingest README.md
texo doctor --deep
```

`texo install` is idempotent. It writes the client-neutral MCP and advisory-hook
manifests under `.texo/`, detects existing Codex, Claude Code, and Cursor
project configuration, merges only Texo-owned entries, and adds one managed
block to `AGENTS.md`. Preview it with `--dry-run --json`; remove only those
managed entries with `texo uninstall`, or select one adapter with
`texo uninstall --client <codex|claude|cursor>`. Texo records which empty
adapter files it created, so uninstall removes those files without deleting
pre-existing user-owned shells.

From this source checkout, a complete clean demo is:

```sh
just demo-fresh
just demo-intelligence
```

Release evidence is reproducible through `just measure-intelligence` and the
literal external-fixture compatibility matrix through `just verify-old-store`.
The latter intentionally fails when the immutable 0.9 fixture archive is not
available; `test-invariants` is substrate smoke coverage, not old-store proof.

Multi-workspace scopes live in `.texo/config.toml` under
`[workspaces.<id>]`. Each workspace carries a normalized journal map:

```toml
[workspaces.demo]
primary_journal = "canonical"
docs_glob = "docs/**/*.md"

[workspaces.demo.journals.canonical]
role = "canonical"
store_path = ".texo/store"

[workspaces.demo.journals.codex]
role = "replica"
store_path = ".texo/replicas/codex"
source_journal = "canonical"
replica_mode = "imported_read_model"
```

Use `--workspace <id>` on any CLI command. Canonical journals own writes;
replicas are independently materialized, journal-affine read surfaces.

## Commands

- `texo init --workspace demo` creates `.texo/config.toml` and the store.
- `texo ingest <path>` records source and claim events.
- `texo claims`, `texo agent-context`, `texo check-staleness <path>`,
  `texo conflicts`, and `texo verify` are replayed read surfaces.
- `texo relate` runs the semantic relation pass when `TEXO_LLM_API_KEY` is
  present.
- `texo index` freezes Git source and builds a bounded code index; `texo
  reconcile` then evaluates cached, bounded claim↔code proposals and journals
  only policy-accepted exact evidence.
- `texo compile --out public` writes the static onboarding trophy.
- `texo serve` runs the sync HTTP memory-agent server.
- `texo extract <doc.md>` runs the LLM extractor and writes NDJSON.
- `texo session export <id>` writes a lane-journaled transcript to stdout.
- `texo mcp` runs the read-only line-delimited MCP stdio server.
- `texo ops list` and `texo ops describe <name>` discover the typed operation
  surface without searching source code.
- `texo doctor [--deep] [--fix]` composes config, store, projection, gateway,
  and agent-install diagnostics. `--fix` touches only Texo-managed files.
- `texo backup create <dest>` creates a fresh journal/config backup with
  BatPak snapshot evidence; `texo backup verify <dest>` checks it offline;
  `texo --root <fresh-root> backup restore <source>` verifies, copies, verifies
  the restored chain, and atomically publishes a new workspace without caches
  or agent-client configuration. Restore refuses an existing root.
  Creation prints `manifest_hash_hex`: store that value outside the backup and
  pass `--expect-manifest-hash <hex>` to detect coordinated rewrites. Without
  a separately trusted pin, verification detects corruption and incomplete
  publication, not forgery of both data and manifest.

## Agent tools

The MCP catalog stays deliberately small and progressively discloses detail:

1. `get_agent_context` returns bounded current context and freshness evidence.
2. `search_knowledge` performs bounded, cursor-based discovery at one snapshot.
3. `explain_knowledge` expands one item into provenance and transition evidence.
4. `triangulate` checks a workspace-relative target before it is trusted.
5. `get_workspace_status` reports frontier, freshness, and settlement state.

All five tools are read-only. Successful calls include concise text plus
`structuredContent` with `schema`, `data`, `meta`, and `next_actions`; failures
carry commit/retry/resume facts. Installed hooks are also fixed read-only Texo
commands—never workspace-supplied shell commands.

## Single-Crate Map

- `src/events/` - v2 payloads, transition machines, coordinates, IDs.
- `src/claims/` - per-entity projections and deterministic workspace views.
- `src/extract/` - markdown heuristics, LLM extraction, record-once caches.
- `src/semantics/` - OpenAI-compatible semantic backends and chat builders.
- `src/reconcile.rs` - bounded doc↔code candidates and proposal-only policy.
- `src/ops/` - syncbat operation handlers and the Texo effect backend.
- `src/host/` - store opening and sealed hostbat module composition.
- `src/surfaces/cli/` - CLI parsing and renderers.
- `src/surfaces/http/` - hand-rolled sync HTTP/1.1 server/client and SSE.
- `src/surfaces/mcp_stdio.rs` - sync MCP JSON-RPC 2.0 stdio surface.

## Memory Agent

Run:

```sh
cargo run --bin texo -- serve --root . --workspace memory
```

The server exposes:

- `GET /` for the UI (`ui/dist` when present, embedded fallback otherwise).
- `POST /api/chat` for model-backed chat grounded in current claims.
- `GET /api/memory` for the replayed memory snapshot.
- `POST /api/session/end` to ingest a session transcript.
- `GET /api/stream` for LiteShip-compatible SSE journal signals.

Session turns are BatPak lane events. They are durable immediately, hidden from
lane-0 claim projections, and become normal claims only after session-end
transcript ingest. The journal is the session.

## Helios Demo

The messy Helios corpus in `examples/helios/docs/` contains contradictory
deployment, ownership, and storage claims. The semantic pipeline extracts,
relates, supersedes, and compiles it into
[`examples/helios/onboarding.generated.md`](examples/helios/onboarding.generated.md).

```sh
TEXO_LLM_API_KEY=sk-... just demo-helios
```

Record-once caches live under `.texo/cache/`, so cached runs replay without
network. The always-on frozen guard is:

```sh
cargo test --test helios_frozen
```

## What This Is Not

texo is not a database server, consensus system, Slack crawler, Google Docs
clone, vector database, or general-purpose LLM extraction framework. The model
is optional record-once perception at append boundaries. The journal remains
source truth.

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or
the [MIT license](LICENSE-MIT), at your option.
