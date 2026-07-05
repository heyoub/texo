# Agent Guide

## Repo map

- [`crates/texo-core/`](crates/texo-core/) — domain logic, BatPak journal adapter, replay, staleness, semantic trait seams + relate logic
- [`crates/texo-cli/`](crates/texo-cli/) — `texo` binary (incl. `texo relate`)
- [`crates/texo-mcp/`](crates/texo-mcp/) — read-only MCP stdio tools
- [`crates/texo-semantics/`](crates/texo-semantics/) — optional ML backends (OpenAI-compatible hosted API, base-URL overridable — OpenRouter default; local ONNX opt-in)
- [`crates/texo-extract/`](crates/texo-extract/) — LLM extractor binary (`extract_via_cmd` seam) + record-once cache
- [`crates/texo-agent/`](crates/texo-agent/) — memory-agent HTTP server (`texo-agent` binary): chat + live memory UI over the claim-chain, session-end transcript ingest + relate
- [`extensions/vscode/`](extensions/vscode/) — thin diagnostics shell over CLI
- [`sample_sources/`](sample_sources/) — demo markdown inputs
- [`examples/helios/`](examples/helios/) — the messy dogfood corpus + ground truth + committed trophy

## Canonical commands

- `just verify` — fmt, clippy, test-hygiene, cargo-deny, typos, full test suite
- `just test-prop` — property tests with `PROPTEST_CASES=256`
- `just demo` — spec demo flow
- `just demo-fresh` — wipe `.texo` + `public/` then demo (non-zero ingest)
- `just demo-helios` — semantic pipeline end-to-end on the messy Helios corpus (needs `OPENROUTER_API_KEY`)
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
