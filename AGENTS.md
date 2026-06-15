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
- `just demo-fresh` — wipe `.texo` + `public/` then demo (non-zero ingest)
- `just ext-package` — build VS Code `.vsix`
- `just test-invariants` — courtroom invariant tests

## Disk quota (agent / CI)

If builds fail with `Disk quota exceeded (errno 122)`:

```sh
cargo clean                                    # drop target/ artifacts (often multi-GB)
export TMPDIR="$PWD/target/tmp"                # avoid sandbox /tmp quota
mkdir -p "$TMPDIR"
export CARGO_TARGET_DIR="$PWD/target"           # keep cargo output in-repo
```

Also prune `/tmp/cursor-sandbox-cache` if the agent sandbox is full.

## Boundaries

- BatPak imports only in [`crates/texo-core/src/journal/`](crates/texo-core/src/journal/)
- CLI/MCP/extension never import `batpak` directly
- Current state is always rebuilt from journal replay
- MCP tool handlers run BatPak I/O on `spawn_blocking` worker threads

## Product spec

See [`SPEC.md`](SPEC.md) for event catalog, demo narrative, and non-goals.
