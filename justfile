fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

test:
    cargo test --workspace

test-invariants:
    cargo test -p texo-core --test thesis_meta --test replay_truth --test staleness_courtroom --test agent_context --test idempotent_replay

test-hygiene:
    #!/usr/bin/env bash
    set -euo pipefail
    if rg -n 'mock|fake_store' crates/ --glob '!**/deny.toml' 2>/dev/null; then
      echo "test-hygiene: mock/fake BatPak stores are banned in crates/"
      exit 1
    fi
    if rg -n '\.unwrap\(\)' crates/texo-core/src --glob '!**/tests/**' 2>/dev/null; then
      echo "test-hygiene: unwrap() banned in texo-core library code"
      exit 1
    fi

deny:
    cargo deny check

typos:
    typos

verify: fmt-check clippy test-hygiene deny typos test

test-prop:
    PROPTEST_CASES=256 cargo test -p texo-core properties

demo:
    cargo run -p texo-cli -- init --workspace demo
    cargo run -p texo-cli -- ingest sample_sources
    cargo run -p texo-cli -- agent-context --out public/agent-context.json
    cargo run -p texo-cli -- check-staleness sample_sources/stale_onboarding.md --json
    cargo run -p texo-cli -- compile --out public

mcp:
    cargo run -p texo-cli -- mcp
