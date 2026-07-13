# Git and code intelligence

Texo treats Git and code as evidence for claims, not as a second mutable graph
database. The BatPak journal remains authority. Git captures and normalized code
indexes explain where a belief came from; deleting an index may make a query
slower or less precise, but cannot delete or rewrite a durable belief.

## The agent workflow

An agent should use the five read-only MCP tools in this order:

1. Call `get_agent_context` for a bounded task snapshot.
2. Call `search_knowledge` with a narrow query. Keep the returned
   `snapshot_token` for every subsequent call in the investigation.
3. Call `explain_knowledge` before relying on a claim. The response distinguishes
   durable evidence, model-policy provenance, timeline, and uncertainty.
4. Call `triangulate` on a claim, path/span, or exact code symbol when the answer
   depends on code. Treat `supported`, `contradicted`, `mixed`, `incomparable`,
   and `insufficient_evidence` as different states.
5. Call `get_workspace_status` when coverage is partial or settlement is
   incomplete. Ask a developer to run `texo index` or `texo reconcile`; the MCP
   server intentionally has no write tool.

This flow is designed around model limitations: search is bounded, exact source
context stays behind explanation/triangulation, one opaque token prevents
multi-call drift, and missing coverage never looks like proof that something is
absent. Stable `next_actions` let an agent continue without inventing commands.

## What `texo index` freezes

`texo index` resolves `HEAD` once, reads committed bytes from the Git object
database, and pins status comparison to that resolved tree. Its worktree overlay
contains exact modified/untracked bytes and explicit deletions. Semantic index
identity hashes path, object id, mode, and conflict stage rather than volatile
filesystem stat fields, so equivalent checkouts produce the same clean snapshot.

The capture fails closed if the index mutates during the operation. Conflicts,
shallow history, missing/corrupt objects, symlinks, gitlinks, LFS pointers,
unsupported encodings, parser recovery, and byte/item limits surface as closed
coverage-gap values. A conflicted or omitted worktree path cannot silently fall
back to stale committed bytes.

Imported SCIP is the precise tier. Pinned tree-sitter Rust tags are the
syntactic tier. The lexical tier is discovery-only: it covers code/config paths,
keeps one row per name and file, caps rows per file, and excludes prose,
generated bundles, and lockfiles. Analyzer fingerprints and the disposable
artifact schema invalidate old acceleration without rewriting journal facts.

An unchanged default `texo index` authenticates and reuses the existing artifact.
Supplying SCIP or non-default limits requests a fresh build. A missing artifact
is lawfully rebuilt; a present corrupt artifact fails closed.

## Temporal and semantic policy

Git ancestry, not author or committer timestamps, orders committed snapshots.
Distinct branches are concurrent. Dirty overlays are later than their own clean
base but incomparable with other overlays. Concurrent, shallow, or otherwise
unknown source pairs are held back and never sent to a relation model as if one
were newer.

The same rule governs the free lexical replacement heuristic. Words such as
“now” and “moved” propose a supersession; they do not authorize one across an
unknown or concurrent Git boundary. Ingest and session-end return typed held
rows, and a later descendant index resumes an accepted proposal without a paid
call. Workspaces with no Git evidence keep immediate observation-order behavior.

`texo reconcile` uses a model only to propose claim↔code relations. Content-keyed
results are record-once acceleration. Deterministic policy accepts sufficiently
confident support/contradiction evidence, journals exact bounded context plus the
judge fingerprint/cache key/policy version, and leaves claim lifecycle unchanged.
Replay, verify, explanation, and restore make no model call.

## Reproducible proof

Run the complete committed-plus-dirty appliance flow with no API key:

```sh
just demo-intelligence
```

The script creates a fresh Git repository, ingests one claim, indexes a clean
commit, proves unchanged reuse, performs snapshot-bound MCP search/explain,
indexes a modified plus untracked overlay, triangulates an exact Rust symbol,
then creates, pins, restores, reopens, and verifies a cache-free backup. It emits
one `texo.intelligence-demo.v1` JSON report and removes its temporary files.

The remaining release evidence has explicit targets:

```sh
# Requires the immutable external 0.9 fixture archive. Override its location
# with TEXO_09_FIXTURES or pass the directory as the script's first argument.
just verify-old-store

# Emits a machine-readable cold/warm report for the current binary and host.
just measure-intelligence
```

`verify-old-store` is the literal compatibility matrix: six 0.9-written stores
must retain byte-identical claim output and verify cleanly, the corrupted store
must fail at its pinned CRC location, re-ingests must append nothing, and the
single-writer lock must hold. The in-repository `spike_family` tests are BatPak
substrate smoke tests, not a substitute for that external fixture matrix. The
runner fails rather than claiming coverage when the fixture archive is absent.

The same debug build and warm filesystem produced this Texo-on-Texo comparison;
absolute numbers vary by machine, while the before/after inputs and limits were
identical:

| Index behavior | Occurrences | Wall | Peak RSS |
|---|---:|---:|---:|
| Unbounded prose/repeated lexical fallback | 92,525 | 23.18 s | 540,028 KiB |
| Bounded relevant cold index | 17,142 | 4.95 s | 72,864 KiB |
| Unchanged authenticated reuse | 17,142 | 0.51 s | 54,412 KiB |

The final recorded run captured 205 files with 17,142 code occurrences,
syntactic quality, zero coverage gaps, and no truncation. The exact commit,
toolchain, host, binary digest, cold measurements, and warm-reuse proof are in
[`evidence/intelligence-performance.json`](evidence/intelligence-performance.json).
Timing and RSS are informational host observations; deterministic counts,
coverage, boundedness, and authenticated reuse are the product contracts.
Relative to the first measured implementation, this run reduced occurrences by
81.5%, wall time by 78.6%, and peak RSS by 86.5%. Reproduce the report with:

```sh
just measure-intelligence
```

For all correctness anchors and hostile fixtures, see
[`ADR-004-snapshot-evidence-temporal-model.md`](ADR-004-snapshot-evidence-temporal-model.md)
and [`INVARIANTS.md`](INVARIANTS.md).
