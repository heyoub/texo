# texo

Context rot is what happens when teams treat prose as state.

texo is a tiny BatPak-backed claim-chain for internal knowledge. It ingests markdown docs, extracts deliberately simple claims, appends them to a local BatPak journal, and exposes replayed current context to agents through JSON and MCP.

Git tracks code diffs.
texo tracks claim diffs.

The journal is for truth.
The CLI is for agents.
The editor is for humans.
The static page is the trophy case.

## Quickstart

```sh
cargo run -p texo-cli -- init --workspace demo
cargo run -p texo-cli -- ingest sample_sources
cargo run -p texo-cli -- agent-context --out public/agent-context.json
cargo run -p texo-cli -- check-staleness sample_sources/stale_onboarding.md --json
cargo run -p texo-cli -- compile --out public
```

Then run the MCP server:

```sh
cargo run -p texo-cli -- mcp
```

For a clean demo (wipes local `.texo` and regenerated `public/`):

```sh
just demo-fresh
```

Multi-workspace scopes live in `.texo/config.toml` under `[workspaces.<id>]`. Use `--workspace staging` on any CLI command.

## What this is not

texo is not a database server, consensus system, Slack crawler, Google Docs clone, vector database, or LLM extraction framework.

It is a small domain app on top of BatPak.

## The extractor is intentionally dumb

v0 uses simple line heuristics. That is deliberate.

The claim-chain is the point: append, receipt, replay, supersede, prove. A real LLM extractor plugs into the same seam later.

## State-machine framing

Docs are not state.

A claim-chain is closer to a single-writer app-chain for team beliefs: append-only transitions, deterministic replay, hash-committed provenance, and projections for humans and agents.

No consensus is claimed. One workspace scope has one local writer. The output frontier means “replayed through local store sequence N,” not “globally agreed by a network.”
