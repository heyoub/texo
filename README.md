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

## Demo: the messy Helios corpus (1/5 → 5/5)

The honest test isn't a happy-path doc — it's seven contradictory, half-rotted markdown files where the *truth has moved* and a later **noise** line buries the real decision. `examples/helios/docs/` is exactly that: the deploy day changes Friday → Wednesday → Tuesday across three docs, release approval moves Alice → Bob in a raw meeting dump, storage flips Postgres → BatPak, and a rogue partner runbook contradicts the release schedule.

Dropped on that corpus, the dumb v0 heuristic **inverts the truth** (it scored 1/5 — a noise line supersedes the real decision). The semantic pipeline turns it into the correct current context:

```sh
OPENROUTER_API_KEY=sk-or-... just demo-helios
```

That runs the real pipeline end to end — AST segmentation → LLM extraction (`texo-extract`) → faithfulness gate → embed + LLM relation-judge (`texo relate`) → journal → compile — and prints the current claims + conflicts. The generated onboarding ([`examples/helios/onboarding.generated.md`](examples/helios/onboarding.generated.md)) is the trophy:

```
## Current claims
  - Deploys moved to Tuesday.                 (04_release_runbook.md:8)
  - Bob owns release approval now.            (05_meeting_dump.md:11)
  - Postgres stays as the relational metadata store for tenant config.
  - The event table was replaced with BatPak's content-addressed log.

## Stale claims (do not trust)
  - "Deploys happen on Friday."     superseded …      (→ the Tuesday chain)
  - "Deploys moved to Wednesday."   superseded by …   (Tuesday)
  - "Alice owns release approval."  superseded by …   (Bob)
  - "The platform uses Postgres for storage."  superseded …  (BatPak/ADR-019)

## Conflicts (unresolved — both claimed, neither wins)
  - "Releases happen on Monday." vs "Releases go out on Friday."
```

Every claim carries a receipt and a source line; "stale" and "conflict" are computed from the journal, not guessed. The model runs **once** at ingest (record-once boundary) and its outputs are cached content-addressed, so replay/compile are deterministic and re-runs are instant. Design rationale + the measured findings that shaped it are in [ADR-001](ADR-001-semantic-pipeline.md).

> Needs an `OPENROUTER_API_KEY`. The first run calls the models (a few minutes) and fills the cache; later runs replay from cache. Models are configurable (`OPENROUTER_EXTRACTOR_MODEL`, `OPENROUTER_RELATER_MODEL`) — Claude for prod, free models for testing.
>
> The backend is any **OpenAI-compatible endpoint**, not OpenRouter specifically: `OPENROUTER_BASE_URL` overrides the host (e.g. Qwen via DashScope compatible mode), and every role's model is overridable (`OPENROUTER_EXTRACTOR_MODEL`, `OPENROUTER_RELATER_MODEL`, `OPENROUTER_EMBED_MODEL`, `OPENROUTER_NLI_MODEL`, `OPENROUTER_RERANK_MODEL`). Qwen Cloud setup lives in [HACKATHON.md](HACKATHON.md).
>
> **Trust note:** `extractor_cmd` is **trusted local code** that `texo ingest` executes. Review `.texo/config.toml` before ingesting an untrusted repo.
>
> v0 records `extractor_kind` + source line on each claim. Per-model provenance and source-span byte offsets are the next event-schema revision, tracked in [ROADMAP.md](ROADMAP.md).

## What this is not

texo is not a database server, consensus system, Slack crawler, or Google Docs clone. It is **not a general-purpose LLM-extraction framework or a vector database**: the optional semantic pipeline is a *record-once perception layer* that writes claims into the chain — the **journal stays source truth**, and the heuristic extractor is the default. The model is perception at a record boundary, not the source of truth.

It is a small domain app on top of BatPak.

## The extractor is intentionally dumb (by default)

v0 uses simple line heuristics. That is deliberate.

The claim-chain is the point: append, receipt, replay, supersede, prove. A real LLM extractor plugs into the same seam — see the optional semantic pipeline below.

Deferred and future work is tracked in [ROADMAP.md](ROADMAP.md).

## State-machine framing

Docs are not state.

A claim-chain is closer to a single-writer app-chain for team beliefs: append-only transitions, deterministic replay, hash-committed provenance, and projections for humans and agents.

No consensus is claimed. One workspace scope has one local writer. The output frontier means “replayed through local store sequence N,” not “globally agreed by a network.”

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or the [MIT license](LICENSE-MIT), at your option.
