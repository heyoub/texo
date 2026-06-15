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
| `ClaimRecorded` | Heuristic claim extracted from a source line |
| `ClaimSuperseded` | Old claim replaced by new (same subject) |
| `ConflictOpened` | Two current claims contradict (explicit heuristic) |
| `OnboardingCompiled` | Audit trail when static compile runs |

All appends go through BatPak; every commit verifies `AppendReceipt` before surfacing `ReceiptView`.

Extractors compose at the ingest seam: default `heuristic-v1`, optional per-workspace `extractor_cmd` (NDJSON subprocess), or test injection via `ExtractClaimsFn` — no trait hierarchy.

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

- **CLI** (`texo`) — ingest, claims, staleness, compile, verify
- **MCP** — read-only tools over replay (spawn_blocking for BatPak I/O)
- **VS Code extension** — thin shell over CLI diagnostics
- **Static compile** — `public/` trophy case (onboarding, claims JSON, index)

## Non-goals (v0)

- Database server, consensus, Slack crawler, Google Docs clone
- Vector database or semantic search
- LLM extraction framework (heuristic extractor is intentional)
- BatPak projection reactor framework or distributed replication

## Invariants

See [`INVARIANTS.md`](INVARIANTS.md) for the full map. Key laws:

- Replay errors propagate (no silent partial state)
- Receipts verify against store after append
- Compile journals `OnboardingCompiled`
- Conflicts are contradictory **current** claims, not supersession edges

## Architecture

See [`ARCHITECTURE.md`](ARCHITECTURE.md) and [`AGENTS.md`](AGENTS.md).
