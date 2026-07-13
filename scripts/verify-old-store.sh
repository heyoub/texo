#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
texo=${TEXO:-"$repo_root/target/debug/texo"}
fixtures=${1:-${TEXO_09_FIXTURES:-"$HOME/.cache/texo-bench/fixtures-0.9"}}
for command in jq cmp grep; do
  command -v "$command" >/dev/null || {
    echo "required command is unavailable: $command" >&2
    exit 1
  }
done
[[ -x "$texo" ]] || {
  echo "build Texo first or set TEXO to an executable" >&2
  exit 1
}
[[ -f "$fixtures/manifest.json" ]] || {
  echo "0.9 fixture manifest is unavailable: $fixtures/manifest.json" >&2
  exit 1
}
jq -e '.batpak_family_version == "0.9.0" and (.fixtures | length == 7)' \
  "$fixtures/manifest.json" >/dev/null

scratch=$(mktemp -d "${TMPDIR:-/tmp}/texo-old-store.XXXXXX")
server_pid=
cleanup() {
  if [[ -n "$server_pid" ]]; then
    kill -TERM "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$scratch"
}
trap cleanup EXIT

verified=0
for name in empty claims claims-twin conflict session self; do
  "$texo" --root "$fixtures/$name" --workspace demo claims --json \
    >"$scratch/$name.claims.json"
  cmp "$scratch/$name.claims.json" "$fixtures/$name.claims.snapshot.json"
  "$texo" --root "$fixtures/$name" --workspace demo verify --json \
    >"$scratch/$name.verify.json"
  jq -e '.journal_ok and .projection_ok and .transitions_ok and (.errors | length == 0)' \
    "$scratch/$name.verify.json" >/dev/null
  verified=$((verified + 1))
done

if "$texo" --root "$fixtures/corrupted" --workspace demo verify --json \
  >"$scratch/corrupted.out" 2>"$scratch/corrupted.err"; then
  echo "corrupted 0.9 fixture unexpectedly verified" >&2
  exit 1
fi
grep -Eiq 'CRC mismatch in segment 4 at offset 45121' "$scratch/corrupted.err"
verified=$((verified + 1))

reingest_noops=0
while IFS='|' read -r name source; do
  copy="$scratch/reingest-$name"
  cp -a "$fixtures/$name/." "$copy/"
  before=$("$texo" --root "$copy" --workspace demo stats --json | jq -er '.events_total')
  "$texo" --root "$copy" --workspace demo ingest "$source" --json \
    >"$scratch/$name.reingest.json"
  after=$("$texo" --root "$copy" --workspace demo stats --json | jq -er '.events_total')
  jq -e '.events_appended == 0 and .sources_observed == 0 and .claims_recorded == 0' \
    "$scratch/$name.reingest.json" >/dev/null
  [[ "$before" == "$after" ]]
  reingest_noops=$((reingest_noops + 1))
done <<EOF
claims|$repo_root/examples/helios/docs
claims-twin|$repo_root/examples/helios/docs
conflict|$fixtures/conflict-src
self|$fixtures/self-corpus-src
EOF

lock_root="$scratch/lock"
cp -a "$fixtures/empty/." "$lock_root/"
"$texo" --root "$lock_root" --workspace demo serve --addr 127.0.0.1:0 \
  >"$scratch/lock-server.log" 2>&1 &
server_pid=$!
sleep 0.3
kill -0 "$server_pid"
if "$texo" --root "$lock_root" --workspace demo claims --json \
  >"$scratch/lock.out" 2>"$scratch/lock.err"; then
  echo "second writer unexpectedly opened the locked 0.9 store" >&2
  exit 1
fi
grep -Eiq 'already locked|could not acquire mutable access' "$scratch/lock.err"
kill -TERM "$server_pid"
wait "$server_pid"
server_pid=

jq -n \
  --arg fixtures "$fixtures" \
  --arg generator "$(jq -r '.generator_git_sha' "$fixtures/manifest.json")" \
  --argjson verified "$verified" \
  --argjson reingest_noops "$reingest_noops" \
  '{schema:"texo.old-store-proof.v1",fixture_family:"batpak-0.9.0",fixture_root:$fixtures,generator_git_sha:$generator,stores_verified:$verified,claims_byte_identical:6,corruption_fail_closed:true,reingest_noops:$reingest_noops,single_writer_lock:true}'
