#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
texo=${TEXO:-"$repo_root/target/debug/texo"}
if [[ ! -x "$texo" ]]; then
  echo "build Texo first or set TEXO to an executable" >&2
  exit 1
fi
for command in git jq /usr/bin/time; do
  command -v "$command" >/dev/null || {
    echo "required command is unavailable: $command" >&2
    exit 1
  }
done

scratch=$(mktemp -d "${TMPDIR:-/tmp}/texo-intelligence-e2e.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
workspace="$scratch/workspace"
backup="$scratch/backup"
restored="$scratch/restored"
mkdir -p "$workspace/docs" "$workspace/src"
git -C "$workspace" init -q
git -C "$workspace" config user.name "Texo Demo"
git -C "$workspace" config user.email "texo-demo@example.invalid"
printf '%s\n' 'Decision: retry limit is four attempts.' >"$workspace/docs/retries.md"
printf '%s\n' 'pub fn retry_limit() -> usize { 4 }' >"$workspace/src/config.rs"
git -C "$workspace" add docs/retries.md src/config.rs
git -C "$workspace" commit -qm "record retry policy"

"$texo" --root "$workspace" --workspace demo init >/dev/null
"$texo" --root "$workspace" --workspace demo ingest docs/retries.md --json \
  >"$scratch/ingest.json"
/usr/bin/time -f '{"wall_seconds":%e,"max_rss_kib":%M}' -o "$scratch/clean-time.json" \
  "$texo" --root "$workspace" --workspace demo index --json >"$scratch/clean-index.json"
clean_snapshot=$(jq -er '.source.snapshot_id' "$scratch/clean-index.json")
jq -e '.source.dirty == false and .code.coverage.truncated == false' \
  "$scratch/clean-index.json" >/dev/null

"$texo" --root "$workspace" --workspace demo index --json >"$scratch/reindex.json"
jq -e --arg snapshot "$clean_snapshot" \
  '.source.snapshot_id == $snapshot and .source.already_indexed == true' \
  "$scratch/reindex.json" >/dev/null

mcp_call_at() {
  local root=$1
  local request=$2
  printf '%s\n' "$request" |
    "$texo" --root "$root" --workspace demo mcp |
    jq -e 'select(.id == 1)'
}

mcp_call() {
  local request=$1
  mcp_call_at "$workspace" "$request"
}

search=$(mcp_call '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_knowledge","arguments":{"query":"retry","limit":10}}}')
snapshot_token=$(jq -er '.result.structuredContent.meta.snapshot.token' <<<"$search")
claim_id=$(jq -er '.result.structuredContent.data.results[] | select(.kind == "claim") | .claim.claim_id' <<<"$search" | head -n1)
symbol=$(jq -er '.result.structuredContent.data.results[] | select(.kind == "code" and (.occurrence.path | startswith("src/"))) | .occurrence.symbol' <<<"$search" | head -n1)
explain_request=$(jq -cn --arg claim "$claim_id" --arg snapshot "$snapshot_token" \
  '{jsonrpc:"2.0",id:1,method:"tools/call",params:{name:"explain_knowledge",arguments:{claim_id:$claim,snapshot_token:$snapshot}}}')
explained=$(mcp_call "$explain_request")
jq -e --arg snapshot "$snapshot_token" \
  '.result.structuredContent.data.snapshot.token == $snapshot' <<<"$explained" >/dev/null

printf '%s\n' 'pub fn retry_limit() -> usize { 5 }' >"$workspace/src/config.rs"
printf '%s\n' 'pub fn retry_budget() -> usize { retry_limit() }' >"$workspace/src/retry.rs"
/usr/bin/time -f '{"wall_seconds":%e,"max_rss_kib":%M}' -o "$scratch/dirty-time.json" \
  "$texo" --root "$workspace" --workspace demo index --json >"$scratch/dirty-index.json"
dirty_snapshot=$(jq -er '.source.snapshot_id' "$scratch/dirty-index.json")
jq -e --arg clean "$clean_snapshot" \
  '.source.dirty == true and .source.snapshot_id != $clean and .code.coverage.truncated == false' \
  "$scratch/dirty-index.json" >/dev/null

dirty_search=$(mcp_call '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_knowledge","arguments":{"query":"retry_limit","limit":10}}}')
dirty_token=$(jq -er '.result.structuredContent.meta.snapshot.token' <<<"$dirty_search")
triangulate_request=$(jq -cn --arg symbol "$symbol" --arg snapshot "$dirty_token" \
  '{jsonrpc:"2.0",id:1,method:"tools/call",params:{name:"triangulate",arguments:{target:{kind:"symbol",symbol:$symbol},snapshot_token:$snapshot}}}')
triangulated=$(mcp_call "$triangulate_request")
jq -e --arg snapshot "$dirty_token" \
  '.result.structuredContent.data.snapshot.token == $snapshot and (.result.structuredContent.data.coverage.gaps | type == "array")' \
  <<<"$triangulated" >/dev/null

"$texo" --root "$workspace" --workspace demo backup create "$backup" --json \
  >"$scratch/backup.json"
manifest_hash=$(jq -er '.manifest_hash_hex' "$scratch/backup.json")
"$texo" --root "$restored" backup restore "$backup" \
  --expect-manifest-hash "$manifest_hash" --json >"$scratch/restore.json"
"$texo" --root "$restored" verify --json >"$scratch/restored-verify.json"
jq -e '.chain_verified == true' "$scratch/restore.json" >/dev/null
jq -e '.journal_ok and .projection_ok and .transitions_ok' \
  "$scratch/restored-verify.json" >/dev/null
restored_explain_request=$(jq -cn --arg claim "$claim_id" \
  '{jsonrpc:"2.0",id:1,method:"tools/call",params:{name:"explain_knowledge",arguments:{claim_id:$claim}}}')
restored_explained=$(mcp_call_at "$restored" "$restored_explain_request")
jq -e --arg claim "$claim_id" \
  '.result.structuredContent.data.card.claim_id == $claim and (.result.structuredContent.data.evidence | length > 0)' \
  <<<"$restored_explained" >/dev/null

jq -n \
  --arg clean_snapshot "$clean_snapshot" \
  --arg dirty_snapshot "$dirty_snapshot" \
  --arg snapshot_token "$snapshot_token" \
  --arg dirty_token "$dirty_token" \
  --arg claim_id "$claim_id" \
  --arg symbol "$symbol" \
  --arg manifest_hash "$manifest_hash" \
  --slurpfile clean_index "$scratch/clean-index.json" \
  --slurpfile dirty_index "$scratch/dirty-index.json" \
  --slurpfile clean_time "$scratch/clean-time.json" \
  --slurpfile dirty_time "$scratch/dirty-time.json" \
  '{schema:"texo.intelligence-demo.v1",clean:{snapshot_id:$clean_snapshot,snapshot_token:$snapshot_token,sources:$clean_index[0].source.sources_captured,occurrences:$clean_index[0].code.coverage.occurrences,wall_seconds:$clean_time[0].wall_seconds,max_rss_kib:$clean_time[0].max_rss_kib},dirty:{snapshot_id:$dirty_snapshot,snapshot_token:$dirty_token,sources:$dirty_index[0].source.sources_captured,occurrences:$dirty_index[0].code.coverage.occurrences,wall_seconds:$dirty_time[0].wall_seconds,max_rss_kib:$dirty_time[0].max_rss_kib},agent_read:{claim_id:$claim_id,symbol:$symbol,snapshot_consistent:true},restore:{manifest_hash_hex:$manifest_hash,chain_verified:true,caches_restored:false,durable_explanation:true}}'
