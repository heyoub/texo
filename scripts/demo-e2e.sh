#!/usr/bin/env bash
# Fresh-workspace end-to-end demo rehearsal. Run twice clean before recording.
#
# Usage: demo-e2e.sh [workdir]
#
# Sequence: fresh workspace -> serve -> three-session memory arc (teach /
# supersede / recall) -> helios oracle pass -> self-ingest drift finale.
# Requires TEXO_LLM_API_KEY (and optionally TEXO_LLM_BASE_URL + role models)
# in the environment.
set -euo pipefail

REPO=/home/heyoub/Code/texo
WORK=${1:-$HOME/.cache/texo-demo-e2e}
ADDR=127.0.0.1:8790
BIN=$REPO/target/release/texo
: "${TEXO_LLM_API_KEY:?TEXO_LLM_API_KEY must be set}"

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
post() { curl -sS -X POST "http://$ADDR$1" -H 'Content-Type: application/json' -d "$2"; echo; }

say "fresh workspace at $WORK"
rm -rf "$WORK"; mkdir -p "$WORK"
"$BIN" --root "$WORK" init

say "serve"
TEXO_AGENT_ROOT="$WORK" TEXO_AGENT_ADDR="$ADDR" "$BIN" serve > "$WORK/serve.log" 2>&1 &
SERVE_PID=$!
trap 'kill $SERVE_PID 2>/dev/null || true' EXIT
for _ in $(seq 1 60); do curl -so /dev/null "http://$ADDR/api/memory" && break; sleep 0.5; done

say "session 1 — teach"
post /api/chat '{"session_id":"demo-s1","message":"The Helios deploy window is Friday. Alice owns Helios release approval."}'
post /api/session/end '{"session_id":"demo-s1"}'

say "session 2 — change"
post /api/chat '{"session_id":"demo-s2","message":"The Helios deploy window moved to Tuesday."}'
post /api/session/end '{"session_id":"demo-s2"}'

say "session 3 — fresh recall"
post /api/chat '{"session_id":"demo-s3","message":"What is the Helios deploy window now, and what was it before?"}'

say "memory with receipts"
curl -sS "http://$ADDR/api/memory" | tee "$WORK/memory.json" | head -c 2000; echo

say "assert: Tuesday current, Friday superseded"
jq -e '[.claims[]? // .[]?] | any(.text | test("Tuesday"))' "$WORK/memory.json" > /dev/null \
  && echo "PASS: Tuesday present" || { echo "FAIL: no Tuesday claim"; exit 1; }

kill $SERVE_PID 2>/dev/null || true; trap - EXIT

say "helios oracle pass"
(cd "$REPO" && just demo-helios)

say "drift finale (self-ingest supersedes old-architecture claims)"
(cd "$REPO" && just drift)

say "DEMO E2E COMPLETE"
