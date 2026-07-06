# texo

Supersedes: see [ADR-003](ADR-003-single-crate-rebuild.md).

texo is claim-chain memory for teams and agents. It ingests markdown, records
typed claims in a BatPak journal, supersedes stale claims with receipts, and
replays deterministic current context through CLI, HTTP, SSE, MCP, and static
compile surfaces.

Git tracks code diffs. texo tracks claim diffs.

## Quickstart

```sh
cargo run --bin texo -- init --workspace demo
cargo run --bin texo -- ingest sample_sources
cargo run --bin texo -- agent-context --out public/agent-context.json
cargo run --bin texo -- check-staleness sample_sources/stale_onboarding.md --json
cargo run --bin texo -- compile --out public
```

For a clean local run:

```sh
just demo-fresh
```

Multi-workspace scopes live in `.texo/config.toml` under
`[workspaces.<id>]`. Use `--workspace <id>` on any CLI command.

## Commands

- `texo init --workspace demo` creates `.texo/config.toml` and the store.
- `texo ingest <path>` records source and claim events.
- `texo claims`, `texo agent-context`, `texo check-staleness <path>`,
  `texo conflicts`, and `texo verify` are replayed read surfaces.
- `texo relate` runs the semantic relation pass when `OPENROUTER_API_KEY` is
  present.
- `texo compile --out public` writes the static onboarding trophy.
- `texo serve` runs the sync HTTP memory-agent server.
- `texo extract <doc.md>` runs the LLM extractor and writes NDJSON.
- `texo session export <id>` writes a lane-journaled transcript to stdout.
- `texo mcp` runs the read-only line-delimited MCP stdio server.

## Single-Crate Map

- `src/events/` - v2 payloads, transition machines, coordinates, IDs.
- `src/claims/` - per-entity projections and deterministic workspace views.
- `src/extract/` - markdown heuristics, LLM extraction, record-once caches.
- `src/semantics/` - OpenAI-compatible semantic backends and chat builders.
- `src/ops/` - syncbat operation handlers and the Texo effect backend.
- `src/host/` - store opening, op composition, canonical fingerprints.
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
OPENROUTER_API_KEY=sk-... just demo-helios
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
