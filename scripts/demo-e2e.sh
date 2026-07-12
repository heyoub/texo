#!/usr/bin/env bash
# Fresh-workspace agent-appliance rehearsal. The default path is entirely
# heuristic and is run twice; live semantic quality remains an optional,
# explicitly key-gated extension.
set -euo pipefail

REPO=${REPO:-/home/heyoub/Code/texo}
WORK_BASE=${1:-$HOME/.cache/texo-demo-e2e}
BIN=${TEXO_BIN:-$REPO/target/release/texo}

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

if [[ ! -x "$BIN" ]]; then
  say "build release binary"
  (cd "$REPO" && cargo build --release --bin texo)
fi

for rehearsal in 1 2; do
  WORK="$WORK_BASE-$rehearsal"
  BACKUP="$WORK_BASE-$rehearsal.backup"
  say "rehearsal $rehearsal: fresh workspace"
  rm -rf "$WORK" "$BACKUP"
  mkdir -p "$WORK/sessions"

  "$BIN" --root "$WORK" --workspace demo install --client all --json \
    > "$WORK/install.json"
  "$BIN" --root "$WORK" --workspace demo install --client all --json \
    | jq -e 'all(.changes[]; .action == "unchanged")' > /dev/null

  say "session 1: teach"
  printf '%s\n' \
    '# Session 1' \
    '' \
    'Decision: The Helios deploy window is Friday.' \
    '' \
    'Decision: Alice owns Helios release approval.' \
    > "$WORK/sessions/01-teach.md"
  "$BIN" --root "$WORK" ingest sessions/01-teach.md --json > "$WORK/ingest-1.json"

  say "session 2: change"
  printf '%s\n' \
    '# Session 2' \
    '' \
    'Decision: The Helios deploy window is now Tuesday instead of Friday.' \
    > "$WORK/sessions/02-change.md"
  "$BIN" --root "$WORK" ingest sessions/02-change.md --json > "$WORK/ingest-2.json"

  say "session 3: agent recall"
  "$BIN" --root "$WORK" agent-context --json > "$WORK/context.json"
  "$BIN" --root "$WORK" claims --json > "$WORK/claims.json"
  jq -e 'any(.[]; (.text | test("Tuesday")) and .status == "current")' \
    "$WORK/claims.json" > /dev/null
  jq -e 'any(.[]; (.text | test("Friday")) and .status == "superseded")' \
    "$WORK/claims.json" > /dev/null
  jq -e 'any(.[]; (.text | test("Alice owns")) and .status == "current")' \
    "$WORK/claims.json" > /dev/null

  say "five-tool MCP catalog"
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
    | "$BIN" --root "$WORK" --workspace demo mcp > "$WORK/mcp.jsonl"
  jq -se '.[1].result.tools | length == 5' "$WORK/mcp.jsonl" > /dev/null

  say "doctor and evidence-backed backup"
  "$BIN" --root "$WORK" doctor --deep --json > "$WORK/doctor.json"
  jq -e '.status == "healthy"' "$WORK/doctor.json" > /dev/null
  "$BIN" --root "$WORK" backup create "$BACKUP" --json > "$WORK/backup.json"
  "$BIN" backup verify "$BACKUP" --json > "$WORK/backup-verify.json"
  jq -e '.verified == true' "$WORK/backup-verify.json" > /dev/null
  say "rehearsal $rehearsal passed"
done

if [[ -n "${TEXO_LLM_API_KEY:-}" ]]; then
  say "optional live semantic Helios quality gate"
  (cd "$REPO" && just demo-helios)
else
  say "semantic quality gate skipped (set TEXO_LLM_API_KEY to opt in)"
fi

say "DEMO E2E COMPLETE: two fresh heuristic appliance runs passed"
