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
| 9 | 1 | `RelationJudgedV1` | provider-neutral logical relation pair |
| 10 | 1 | `RelationDeferredV1` | provider-neutral logical relation pair |
| 11 | 1 | `SourceSnapshotRecordedV1` | `source-snapshot:{snapshot_id}` |
| 12 | 1 | `EvidenceOccurrenceRecordedV1` | `evidence:{occurrence_id}` |
| 13 | 1 | `ClaimEvidenceLinkedV1` | `claim:{claim_id}` |
| 14 | 1 | `CodeIndexRecordedV1` | `code-index:{index_id}` |
| 15 | 1 | `SourceSnapshotRelationV1` | directed source-snapshot pair |

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
  `agent-context`, `index`, `compile`, `relate`, `conflicts`, `verify`, `serve`,
  `extract`, `session export`, `host fingerprint`, `ops`, `install`,
  `uninstall`, `hook`, `doctor`, `backup`, and `mcp`.
  `index` freezes Git source and builds code intelligence in one invocation;
  `--scip` supplies an optional workspace-local precise index.
- HTTP: `GET /`, `GET /api/host`, `POST /api/chat`, `GET /api/memory`,
  `POST /api/session/end`, and `GET /api/stream`. Request heads are capped at
  8 KiB, POST bodies at 1 MiB, and unsupported transfer encoding returns 501.
- SSE: `hello` signal on connect, `journal` signal per workspace event,
  keep-alive comments on quiet ticks, and resume via `Last-Event-ID` header
  or `lastEventId` query param replays workspace events after the cursor.
- MCP: line-delimited JSON-RPC 2.0 stdio with `initialize`, `tools/list`, and
  `tools/call` for five read-only tools: `get_agent_context`,
  `search_knowledge`, `explain_knowledge`, `triangulate`, and
  `get_workspace_status`. Successful calls carry output-schema-validated
  structured content, one reusable snapshot token, explicit coverage, and
  bounded next actions.
- Triangulation returns a closed answer state (`supported`, `contradicted`,
  `stale`, `unverified`, or `incomparable`), exact bounded evidence when
  journaled, typed uncertainty, and coverage. Search hits alone are never
  promoted to evidence.
- `search_knowledge` returns one bounded, snapshot-bound union of semantic
  claims and code occurrences. Claim filters exclude code rows by construction;
  opaque cursors are bound to the query, filters, and snapshot.
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
