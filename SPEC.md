# texo product spec

Supersedes: see [ADR-003](ADR-003-single-crate-rebuild.md).

## Thesis

Teams treat markdown as state. It is not. Prose rots; claims supersede each
other. texo is a local claim-chain on BatPak: ingest docs, append typed events
with receipts, replay deterministic projections, and expose current context to
agents.

## Event Catalog V2

All domain events are BatPak payloads in category `0xE`.

| Type | Version | Payload | Entity |
|---|---:|---|---|
| 1 | 2 | `SourceObservedV2` | `source:{source_id}` |
| 2 | 2 | `ClaimRecordedV2` | `claim:{claim_id}` |
| 3 | 2 | `ClaimSupersededV2` | `claim:{old_claim_id}` |
| 4 | 2 | `ConflictOpenedV2` | `conflict:{conflict_id}` |
| 5 | 2 | `OnboardingCompiledV2` | `projection:onboarding` |
| 6 | 1 | `ConflictResolvedV2` | `conflict:{conflict_id}` |
| 7 | 1 | `WorkspaceInitializedV2` | `workspace-meta:{workspace_id}` |
| 8 | 1 | `SessionTurnV1` | `session:{session_id}` on `session_lane(id)` |

Claim and conflict state changes carry `TransitionRecordV1` evidence with a
deterministic blake3 transition id and explicit causes.

## Demo Narrative

1. `deploy_schedule.md` says deploys happen on Friday.
2. `decision_deploy_day.md` says deploys moved to Tuesday.
3. Ingest records sources and claims.
4. Supersession records transition evidence from the Friday claim to Tuesday.
5. Replay marks Friday superseded and Tuesday current.
6. Staleness, agent context, MCP, and static compile report current state with
   provenance and receipts.

## Surfaces

- CLI: `init`, `ingest`, `claims`, `supersede`, `check-staleness`,
  `agent-context`, `compile`, `relate`, `conflicts`, `verify`, `serve`,
  `extract`, `session export`, `host fingerprint`, and `mcp`.
- HTTP: `GET /`, `GET /api/host`, `POST /api/chat`, `GET /api/memory`,
  `POST /api/session/end`, and `GET /api/stream`. Request heads are capped at
  8 KiB, POST bodies at 1 MiB, and unsupported transfer encoding returns 501.
- SSE: `hello` signal on connect, `journal` signal per workspace event,
  keep-alive comments on quiet ticks, and resume via `Last-Event-ID` header
  or `lastEventId` query param replays workspace events after the cursor.
- MCP: line-delimited JSON-RPC 2.0 stdio with `initialize`, `tools/list`, and
  `tools/call` for four read-only tools: `check_staleness`,
  `get_current_claims`, `get_agent_context`, and `explain_claim`.
- Static compile: `onboarding.generated.md`, claims JSON, and index files.

## Semantic Pipeline

The default extractor is heuristic. When configured, `texo extract` runs an
OpenAI-compatible proposer, faithfulness gate, and record-once cache. `texo
relate` embeds current claims, clusters related claims, and asks the relation
judge for supersession/conflict verdicts. Model outputs are cached by content
identity before becoming journal events.

## Non-Goals

- Database server, consensus system, Slack crawler, or Google Docs clone.
- General-purpose vector database or semantic-search engine.
- General-purpose LLM extraction framework.
- Distributed replication.
- Async runtime stack; the rebuilt binary is sync-first by design.

## Test Anchors

See [INVARIANTS.md](INVARIANTS.md) for the invariant map and test anchors.
