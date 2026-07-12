# ADR-004: Snapshot, evidence, and temporal model

Status: accepted for implementation on `agent/snapshot-evidence-intelligence`.

## Context

Texo currently records source observations and extracted claims together, then
orders semantic relation candidates by journal receipt sequence. A second agent
path compares claim `observed_at_ms` values. Neither coordinate establishes
which assertion is newer in a source domain such as Git: receipt order says when
Texo learned a fact, wall time says when a caller observed it, and Git ancestry
says whether one revision descends from another.

Code-aware agent use also requires a stable view across several calls. Without a
snapshot token, an agent can load context from one journal frontier and explain
it against a later worktree. Absence is similarly ambiguous unless results say
which sources and analysis modes were actually covered.

## Decision

### Separate three planes

1. **Evidence plane** — immutable source snapshots and bounded evidence
   occurrences. An occurrence is source support, not a semantic assertion.
2. **Structural plane** — disposable code indexes and versioned structural
   facts. Compiler-produced SCIP is preferred, syntactic analysis is a fallback,
   and bounded lexical discovery is the universal floor.
3. **Belief plane** — existing claims, relation judgments, supersessions,
   conflicts, and holdback. Models propose; deterministic policy decides.

The BatPak journal remains semantic source truth. Any evidence used to justify a
durable belief is represented by a bounded journal payload containing its exact
excerpt, source identity, span, analyzer fingerprint, and content digest. Large
source and code indexes may be retained as content-addressed artifacts, but loss
of those artifacts may reduce coverage only; it cannot change replayed belief
state or erase the evidence excerpt carried by the journal.

### Keep time coordinates distinct

Texo uses the closed `TemporalRelation` result `same | before | after |
concurrent | unknown`. It is derived from the coordinate appropriate to the
source:

- BatPak sequence/HLC orders durable observations and projection frontiers.
- BatPak causation identifies which recorded event caused another.
- Git parent ancestry orders committed revisions.
- An optional asserted-effective time is domain evidence, never a universal
  tie-breaker.
- A frozen index/worktree overlay describes the developer-visible state over a
  resolved base commit.

Git author/committer timestamps are descriptive provenance. They never turn
parallel branches into a total order. Automatic supersession requires `after`
and an authority policy that permits the transition. `concurrent` produces a
conflict or holdback; `unknown` remains unresolved.

### Freeze snapshots before agent reads

An indexed source snapshot resolves a Git ref once, records the resulting object
ID, and freezes the index/worktree overlay bytes before extraction. A
`SnapshotToken` binds:

- workspace id;
- BatPak frontier and anchor event id;
- optional indexed source snapshot id.

Every agent read accepts an optional token and returns the token actually read.
A token that can no longer be served fails with a typed snapshot error rather
than silently reading the latest state. MCP tools remain read-only; snapshot and
index creation are explicit write operations.

### Preserve the five-tool progressive-disclosure surface

The curated catalog remains five tools:

1. `get_agent_context`
2. `search_knowledge`
3. `explain_knowledge`
4. `triangulate`
5. `get_workspace_status`

All tools publish MCP `outputSchema`, return structured snapshot, coverage,
uncertainty, and conditional next-action fields, and keep large evidence behind
bounded resource reads. `triangulate` subsumes Markdown-only staleness checking
by accepting a claim, path, span, or code symbol target.

### Git and code-intelligence rules

- Use `gix` with a minimal feature set and reject untrusted repository config.
- Read committed content as raw object-database blobs. Working-tree filters do
  not participate in committed identity.
- Represent Git SHA-1 and SHA-256 object IDs explicitly.
- Freeze a resolved base commit plus index/worktree overlay; never re-resolve a
  moving ref mid-operation.
- Symlinks are recorded, never followed. Gitlinks, LFS pointers, shallow
  history, conflicts, missing objects, size limits, and unsupported encodings
  become typed coverage gaps.
- Prefer imported SCIP indexes for precise definitions/references. Syntactic
  results advertise parser recovery and grammar fingerprints. Lexical results
  never masquerade as structural certainty.
- Analyzer or model calls never occur during replay or verify.

## Additive schema plan

Existing event payloads and identifiers remain byte-compatible. New information
lands through additive event kinds and projections:

- `SourceSnapshotRecordedV1`
- `EvidenceOccurrenceRecordedV1`
- `ClaimEvidenceLinkedV1`
- `CodeIndexRecordedV1`

Detailed code indexes remain derived artifacts. `ClaimEvidenceLinkedV1` records
only the bounded evidence that affected a belief. Existing claims receive a
deterministic legacy occurrence projection; old stores require no rewrite.

## Laws

1. Replay and verify perform no Git, parser, indexer, or model calls.
2. Journal belief state is explainable without a disposable artifact.
3. Deleting every derived index changes coverage/performance, never belief.
4. One snapshot token never names two different source or journal states.
5. Git-ref movement during indexing cannot change the captured base revision.
6. Concurrent or unknown revisions are never ordered by timestamp.
7. Partial coverage never masquerades as a negative fact or complete result.
8. Precise, syntactic, and lexical evidence remain distinguishable end to end.
9. Existing event bytes, claim ids, and old-store replay remain unchanged.
10. Paid semantic results remain cached and proposal-only until policy accepts
    them.

## Proof obligations

- Property tests for token determinism, Git-DAG comparison, and fold equality.
- Ref-race, force-push, shallow clone, symlink, gitlink, LFS, conflict, malformed
  object, invalid encoding, size-budget, and dirty-overlay fixtures.
- Snapshot-consistent multi-call MCP tests with declared output schemas.
- Artifact deletion and cache deletion preserve replayed beliefs and expose
  degraded coverage.
- SCIP/syntactic/lexical fixtures prove quality labels and parser-error
  propagation.
- Old-store gauntlet, backup/verify, full repository gates, and an end-to-end
  committed-plus-dirty-worktree demonstration.

