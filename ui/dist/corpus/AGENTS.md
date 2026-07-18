# Agent Guide

Supersedes: see [ADR-003](ADR-003-single-crate-rebuild.md).

<!-- codebase-memory-mcp:start -->
# Codebase Knowledge Graph (codebase-memory-mcp)

This project uses codebase-memory-mcp to maintain a knowledge graph of the codebase.
ALWAYS prefer MCP graph tools over grep/glob/file-search for code discovery.

## Priority Order
1. `search_graph` - find functions, classes, routes, variables by pattern
2. `trace_path` - trace who calls a function or what it calls
3. `get_code_snippet` - read specific function/class source code
4. `query_graph` - run Cypher queries for complex patterns
5. `get_architecture` - high-level project summary

## When to fall back to grep/glob
- Searching for string literals, error messages, config values
- Searching non-code files (Dockerfiles, shell scripts, configs)
- When MCP tools return insufficient results
<!-- codebase-memory-mcp:end -->

## Repo Map

- `src/events/` - event schema v2, transitions, coordinates, IDs.
- `src/claims/` - projections, statuses, `WorkspaceView`, session log.
- `src/extract/` - markdown, heuristic and LLM extraction, caches.
- `src/semantics/` - OpenAI-compatible model backends and chat.
- `src/ops/` - syncbat op handlers, effect backend, operation environment.
- `src/host/` - store opening, invocation, fingerprints.
- `src/surfaces/` - CLI, HTTP, SSE, MCP stdio, bootstrap, model edge.
- `tests/` - real-store integration, projection, HTTP, MCP, Helios, goldens.
- `sample_sources/` - small demo corpus.
- `examples/helios/` - dogfood corpus, ground truth, generated trophy.
- `extensions/vscode/` - thin CLI-based diagnostics shell.

## Canonical Commands

- `just verify` - fmt-check, clippy, hygiene, cargo-deny, typos, full tests.
- `just test-invariants` - projection, compile-fail, and BatPak-family spikes.
- `just demo` / `just demo-fresh` - spec demo flow.
- `just demo-helios` - semantic Helios run, requires `TEXO_LLM_API_KEY`
  unless caches fully satisfy it.
- `just drift` - informational texo-on-texo prose audit; always exits 0.
- `cargo run --bin texo -- <command>` - the only runtime binary.

## Rules

- Stay on the single crate; do not reintroduce workspace crates.
- Do not add tokio, reqwest, axum, tower, rmcp, schemars, or anyhow.
- Keep code sync-first and thread names explicit when spawning.
- No `unwrap()`, `panic!`, `todo!`, `unimplemented!`, `dbg!`, or unsanctioned
  stdout/stderr prints.
- Integration tests use real BatPak stores.
- The words banned by `just test-hygiene` must not appear in `src/` or `tests/`.

## Product Spec

See [SPEC.md](SPEC.md), [ARCHITECTURE.md](ARCHITECTURE.md), and
[INVARIANTS.md](INVARIANTS.md).
