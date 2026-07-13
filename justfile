fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --all-targets -- -D warnings

test:
    cargo test

test-invariants:
    cargo test --test projection_laws --test compile_fail --test spike_family

demo-intelligence:
    cargo build --bin texo
    scripts/demo-intelligence-e2e.sh

verify-old-store:
    cargo build --bin texo
    scripts/verify-old-store.sh

measure-intelligence:
    cargo build --bin texo
    scripts/measure-intelligence.sh

test-hygiene:
    #!/usr/bin/env bash
    set -euo pipefail
    if rg -n 'mock|fake_store' src/ tests/ 2>/dev/null; then
      echo "test-hygiene: mock/fake BatPak stores are banned in src/ tests/"
      exit 1
    fi
    if rg -n '\.unwrap\(\)' src/ 2>/dev/null; then
      echo "test-hygiene: unwrap() banned in src/"
      exit 1
    fi

deny:
    cargo deny check

typos:
    typos

verify: fmt-check clippy test-hygiene deny typos test

test-prop:
    PROPTEST_CASES=256 cargo test properties

demo:
    cargo run --bin texo -- init --workspace demo
    cargo run --bin texo -- ingest sample_sources
    cargo run --bin texo -- agent-context --out public/agent-context.json
    cargo run --bin texo -- check-staleness sample_sources/stale_onboarding.md --json || true
    cargo run --bin texo -- compile --out public

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
    : "${TEXO_LLM_API_KEY:?set TEXO_LLM_API_KEY to run the semantic demo}"
    # Build the LLM extractor and the CLI.
    cargo build -q --bin texo
    EXTRACT="$(pwd)/target/debug/texo extract"
    TEXO="./target/debug/texo"
    # Record-once caches live OUTSIDE the wiped journal, so a re-run replays the
    # captured model output instantly and deterministically (first run fills them).
    export TEXO_EXTRACT_CACHE="$(pwd)/.texo/cache/extract"
    export TEXO_RELATE_CACHE="$(pwd)/.texo/cache/relate"
    mkdir -p "$TEXO_EXTRACT_CACHE" "$TEXO_RELATE_CACHE"
    # Fresh journal each run; the caches above are preserved.
    rm -rf .texo/helios-store .texo/config.toml public/helios
    mkdir -p .texo/helios-store public/helios
    cat > .texo/config.toml <<TOML
    default_workspace = "helios"

    [workspaces.helios]
    store_path = ".texo/helios-store"
    docs_glob = "examples/helios/docs/**/*.md"
    extractor_cmd = "$EXTRACT"

    [workspaces.helios.semantics]
    enabled = true
    # Recall-favoring candidate thresholds: the judge is the correctness gate,
    # so a lower floor only costs judge calls (bounded by cluster sizes).
    cosine_threshold = 0.5
    relate_prefilter = 0.5
    TOML
    echo "==> ingest (LLM extraction via texo-extract; first run hits OpenRouter, then cached)"
    "$TEXO" ingest examples/helios/docs
    echo "==> relate (semantic supersession + conflict pass; cached + resumable)"
    set +e
    "$TEXO" relate
    RELATE_STATUS=$?
    set -e
    if [[ "$RELATE_STATUS" -ne 0 && "$RELATE_STATUS" -ne 2 ]]; then exit "$RELATE_STATUS"; fi
    echo "==> compile onboarding -> public/helios/onboarding.generated.md"
    "$TEXO" compile --out public/helios
    echo
    echo "===================== CURRENT CLAIMS (what new hires/agents see) ====================="
    "$TEXO" claims
    echo
    echo "Stale + conflicts are in the trophy:  public/helios/onboarding.generated.md"

ext-package:
    #!/usr/bin/env bash
    set -euo pipefail
    cd extensions/vscode
    if [[ ! -d node_modules ]]; then npm ci; fi
    npm run compile
    npx --yes @vscode/vsce package

mcp:
    cargo run --bin texo -- mcp

# Ingest the repo's own prose and show which architecture claims are
# current vs superseded. Informational: always exits 0. Set
# TEXO_LLM_API_KEY for the semantic relate pass; the heuristic
# supersession runs without it.
drift:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -q --bin texo
    TEXO="$(pwd)/target/debug/texo"
    DIR="$(mktemp -d)"; trap 'rm -rf "$DIR"' EXIT
    mkdir -p "$DIR/docs"
    for f in *.md deploy/README.md hackathon/*.md; do
      case "$f" in *generated*) continue;; esac
      cp "$f" "$DIR/docs/${f//\//__}"
    done
    ( cd "$DIR" \
      && "$TEXO" init --workspace drift \
      && "$TEXO" ingest docs \
      && { [ -n "${TEXO_LLM_API_KEY:-}" ] && TEXO_RELATE_CACHE="$OLDPWD/.texo/cache/relate-drift" "$TEXO" relate || true; } \
      && echo "================ drift: claims ================" \
      && "$TEXO" claims \
      && echo "================ drift: conflicts ================" \
      && { "$TEXO" conflicts || true; } ) || echo "drift: run failed (informational only)"

# Snapshot the drift run as JSON for the UI drift view (ui/public/drift.json,
# picked up by `pnpm build` into ui/dist). Same corpus and flow as `drift`.
drift-ui:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -q --bin texo
    TEXO="$(pwd)/target/debug/texo"
    OUT="$(pwd)/ui/public/drift.json"
    mkdir -p "$(pwd)/ui/public"
    DIR="$(mktemp -d)"; trap 'rm -rf "$DIR"' EXIT
    mkdir -p "$DIR/docs"
    for f in *.md deploy/README.md hackathon/*.md; do
      case "$f" in *generated*) continue;; esac
      cp "$f" "$DIR/docs/${f//\//__}"
    done
    ( cd "$DIR" \
      && "$TEXO" init --workspace drift \
      && "$TEXO" ingest docs \
      && { [ -n "${TEXO_LLM_API_KEY:-}" ] && TEXO_RELATE_CACHE="$OLDPWD/.texo/cache/relate-drift" "$TEXO" relate || true; } \
      && "$TEXO" claims --json > "$OUT" )
    echo "wrote $OUT ($(grep -c 'claim_id' "$OUT") claims)"
