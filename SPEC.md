# texo — product spec (v0)

**Codename:** `ctxvc` (context version control). The shipped binary is `texo`.

## Thesis

Teams treat markdown as state. It is not. Prose rots; claims supersede each other. texo is a tiny **claim-chain** on BatPak: ingest docs, append typed events with receipts, replay deterministic projection, expose current context to agents.

Git tracks code diffs. texo tracks **claim diffs**.

## Demo narrative (Friday → Tuesday)

1. `deploy_schedule.md` says deploys happen on **Friday**.
2. `decision_deploy_day.md` records the team **moved deploys to Tuesday**.
3. Ingest appends sources, claims, and a supersession edge.
4. Replay marks the Friday claim **superseded**; Tuesday claim is **current**.
5. `stale_onboarding.md` still says Friday — `check-staleness` flags exact lines.
6. `agent-context` and MCP tools return replayed current claims with frontier + receipts.

Courtroom tests in `crates/texo-core/tests/` prove each step.

## Event catalog (five payloads)

| Kind | Purpose |
|---|---|
| `SourceObserved` | Hash-committed markdown source |
| `ClaimRecorded` | Claim extracted from a source (heuristic by default, or the optional LLM extractor) |
| `ClaimSuperseded` | Old claim replaced by new (same subject) |
| `ClaimConflictDetected` | Two current claims contradict (heuristic detector, or the semantic relate pass) |
| `OnboardingCompiled` | Audit trail when static compile runs |

All appends go through BatPak; every commit verifies `AppendReceipt` before surfacing `ReceiptView`.

Extractors compose at the ingest seam: default `heuristic-v1`, optional per-workspace `extractor_cmd` (NDJSON subprocess — e.g. the `texo-extract` LLM extractor), or test injection via `ExtractClaimsFn` — no trait hierarchy.

## Semantic pipeline (v1, optional)

Enabled per-workspace via `[semantics]`. AST segmentation → LLM extraction (`texo-extract`) → deterministic faithfulness gate → embedding prefilter + LLM relation-judge (`texo relate`) → journal. The model runs **once** at ingest (record-once boundary); its outputs are cached content-addressed and become journaled events, so replay/compile stay deterministic. Heuristic extraction remains the default. See [`ADR-001`](ADR-001-semantic-pipeline.md).

## Multi-workspace (v1)

Single [`.texo/config.toml`](.texo/config.toml) holds named scopes:

```toml
default_workspace = "demo"

[workspaces.demo]
store_path = ".texo/store"
docs_glob = "sample_sources/**/*.md"

[workspaces.staging]
store_path = ".texo/stores/staging"
docs_glob = "sample_sources/**/*.md"
# extractor_cmd = "python3 scripts/extract-identity.py"
```

CLI: global `--workspace <id>`. VS Code: `texo.workspaceId`.

## Surfaces

- **CLI** (`texo`) — ingest, claims, relate, staleness, compile, conflicts, verify
- **MCP** — read-only tools over replay (spawn_blocking for BatPak I/O)
- **VS Code extension** — thin shell over CLI diagnostics
- **Static compile** — `public/` trophy case (onboarding, claims JSON, index)

## Non-goals (v0)

- Database server, consensus, Slack crawler, Google Docs clone
- A general-purpose vector database or semantic-search engine (embeddings are an internal, optional prefilter for relating claims — not a queryable index)
- A general-purpose LLM-extraction framework (the optional semantic extractor is record-once perception into the claim-chain; the heuristic extractor is the default)
- BatPak projection reactor framework or distributed replication

## Invariants

See [`INVARIANTS.md`](INVARIANTS.md) for the full map. Key laws:

- Replay errors propagate (no silent partial state)
- Receipts verify against store after append
- Compile journals `OnboardingCompiled`
- Conflicts are contradictory **current** claims, not supersession edges

## Architecture

See [`ARCHITECTURE.md`](ARCHITECTURE.md) and [`AGENTS.md`](AGENTS.md).
