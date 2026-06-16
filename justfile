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

demo-fresh:
    #!/usr/bin/env bash
    set -euo pipefail
    rm -rf .texo
    rm -f public/*
    touch public/.gitkeep
    just demo

demo-helios:
    #!/usr/bin/env bash
    set -euo pipefail
    : "${OPENROUTER_API_KEY:?set OPENROUTER_API_KEY to run the semantic demo}"
    # Build the LLM extractor and the CLI.
    cargo build -q -p texo-extract -p texo-cli
    EXTRACT="$(pwd)/target/debug/texo-extract"
    TEXO="./target/debug/texo"
    # Fresh helios workspace wired to the semantic pipeline (LLM extractor + relate).
    rm -rf .texo public/helios
    mkdir -p .texo/helios-store public/helios
    cat > .texo/config.toml <<TOML
    default_workspace = "helios"

    [workspaces.helios]
    store_path = ".texo/helios-store"
    docs_glob = "examples/helios/docs/**/*.md"
    extractor_cmd = "$EXTRACT"

    [workspaces.helios.semantics]
    enabled = true
    TOML
    echo "==> ingest (LLM extraction via texo-extract; first run hits OpenRouter, then cached)"
    "$TEXO" ingest examples/helios/docs
    echo "==> relate (semantic supersession + conflict pass)"
    "$TEXO" relate
    echo "==> compile onboarding"
    "$TEXO" compile --out public/helios
    echo
    echo "===================== CURRENT CLAIMS (what new hires/agents see) ====================="
    "$TEXO" claims
    echo
    echo "===================== CONFLICTS (genuine disagreements surfaced) ======================"
    "$TEXO" conflicts

ext-package:
    #!/usr/bin/env bash
    set -euo pipefail
    cd extensions/vscode
    if [[ ! -d node_modules ]]; then npm ci; fi
    npm run compile
    npx --yes @vscode/vsce package

mcp:
    cargo run -p texo-cli -- mcp
