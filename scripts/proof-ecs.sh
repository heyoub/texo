#!/usr/bin/env bash
# Judge-evidence capture against the deployed ECS box.
#
# Usage: proof-ecs.sh <host-or-ip> [port]
#
# Proves, in one recorded pass: the agent is live on Alibaba Cloud ECS, backed
# by Qwen models via DashScope compatible mode, and demonstrates the memory
# thesis end to end — teach, supersede across sessions, recall with receipts.
# Record the terminal with asciinema/script plus a screen recording.
#
# All facts use explicit third-person subjects (first-person phrasing collides
# with the faithfulness gate by design).
set -euo pipefail

HOST=${1:?host or ip}; PORT=${2:-8787}
BASE="http://$HOST:$PORT"
say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
post() { curl -sS -X POST "$BASE$1" -H 'Content-Type: application/json' -d "$2"; echo; }

say "1/7 health (version, frontier, chat enabled)"
curl -sS "$BASE/api/health"; echo

say "2/7 host fingerprint (op catalog identity)"
curl -sS "$BASE/api/host"; echo

say "3/7 session 1 — teach two facts"
post /api/chat '{"session_id":"proof-s1","message":"The Helios deploy window is Friday. Alice owns Helios release approval."}'
post /api/session/end '{"session_id":"proof-s1"}'

say "4/7 session 2 — one fact changes"
post /api/chat '{"session_id":"proof-s2","message":"The Helios deploy window moved to Tuesday."}'
post /api/session/end '{"session_id":"proof-s2"}'

say "5/7 session 3 — fresh session recall (must cite Tuesday as current, Friday as superseded)"
post /api/chat '{"session_id":"proof-s3","message":"What is the Helios deploy window now, and what was it before?"}'
post /api/session/end '{"session_id":"proof-s3"}'

say "6/7 memory projection — superseded chain with receipts"
curl -sS "$BASE/api/memory"; echo

say "7/7 backend proof — Qwen via DashScope compatible mode (key redacted)"
ssh "root@$HOST" 'systemctl is-active texo-agent && grep -oE "TEXO_LLM_(BASE_URL|[A-Z_]*MODEL)=[^ ]*" /opt/texo/env | sed "s/KEY=.*/KEY=[redacted]/"'

say "DONE — evidence complete"
