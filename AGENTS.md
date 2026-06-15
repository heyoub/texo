# Agent Guide

## Repo map

- [`crates/texo-core/`](crates/texo-core/) — domain logic, BatPak journal adapter, replay, staleness
- [`crates/texo-cli/`](crates/texo-cli/) — `texo` binary
- [`crates/texo-mcp/`](crates/texo-mcp/) — read-only MCP stdio tools
- [`extensions/vscode/`](extensions/vscode/) — thin diagnostics shell over CLI
- [`sample_sources/`](sample_sources/) — demo markdown inputs

## Canonical commands

- `just verify` — fmt, clippy, test-hygiene, cargo-deny, typos, full test suite
- `just test-prop` — property tests with `PROPTEST_CASES=256`
- `just demo` — spec demo flow
- `just test-invariants` — courtroom invariant tests

## Boundaries

- BatPak imports only in [`crates/texo-core/src/journal/`](crates/texo-core/src/journal/)
- CLI/MCP/extension never import `batpak` directly
- Current state is always rebuilt from journal replay
- MCP tool handlers run BatPak I/O on `spawn_blocking` worker threads

## Product spec

See [`SPEC.md`](SPEC.md) for event catalog, demo narrative, and non-goals.
