#!/usr/bin/env bash
# Benchmark-ladder rung harness. DNFs and aborts are data, not failures.
#
# Usage: run-rung.sh RUNG CORPUS [--binary PATH] [--timeout SECS] [--twin] [--label TEXT]
#
# Fresh workspace -> timed ingest -> timed verify -> timed no-op re-ingest ->
# yield distribution -> optional twin-store determinism diff -> metrics.json
# under ~/.cache/texo-bench/runs/<rung>-<label>-<epoch>/.
#
# Model calls are hard-disabled: the api-key env vars are unset for every
# invocation so extraction stays heuristic and deterministic.
set -uo pipefail

RUNG=${1:?rung name}; CORPUS=${2:?corpus path}; shift 2
BIN=${BENCH_BINARY:-/home/heyoub/Code/texo/target/release/texo}
TIMEOUT=2700; TWIN=0; LABEL=dev
while [ $# -gt 0 ]; do
  case "$1" in
    --binary) BIN=$2; shift 2;;
    --timeout) TIMEOUT=$2; shift 2;;
    --twin) TWIN=1; shift 1;;
    --label) LABEL=$2; shift 2;;
    *) echo "unknown arg $1" >&2; exit 2;;
  esac
done

RUN_DIR=$HOME/.cache/texo-bench/runs/${RUNG}-${LABEL}-$(date +%s)
WS=$RUN_DIR/ws; mkdir -p "$WS"
CORPUS=$(readlink -f "$CORPUS")

# every texo invocation goes through here: keyless, timed, exit tolerated
tx() { # tx <tag> <timeout> <args...>
  local tag=$1 to=$2; shift 2
  env -u TEXO_LLM_API_KEY \
    /usr/bin/time -v -o "$RUN_DIR/$tag.time" \
    timeout "$to" "$BIN" "$@" > "$RUN_DIR/$tag.out" 2> "$RUN_DIR/$tag.err"
  echo $? > "$RUN_DIR/$tag.exit"
}
wall() { grep -oP 'Elapsed.*: \K.*' "$RUN_DIR/$1.time" 2>/dev/null | head -1; }
rss()  { grep -oP 'Maximum resident set size.*: \K\d+' "$RUN_DIR/$1.time" 2>/dev/null; }
xc()   { cat "$RUN_DIR/$1.exit" 2>/dev/null || echo -1; }

tx init 60 --root "$WS" init
tx ingest "$TIMEOUT" --root "$WS" ingest "$CORPUS"
tx verify "$TIMEOUT" --root "$WS" verify
tx reingest "$TIMEOUT" --root "$WS" ingest "$CORPUS"
tx claims 600 --root "$WS" claims --json

YIELD="{}"
if [ "$(xc claims)" = "0" ]; then
  YIELD=$(jq '{claims_total: length,
               current: [.[]|select(.status=="current")]|length,
               superseded: [.[]|select(.status=="superseded")]|length,
               sources: ([.[].source.path]|unique|length),
               per_source: ([group_by(.source.path)[]|length] | {min: min, max: max,
                 median: (sort|.[(length/2|floor)])})}' "$RUN_DIR/claims.out" 2>/dev/null || echo '{}')
fi

DETERMINISM="not_run"
if [ "$TWIN" = "1" ] && [ "$(xc ingest)" = "0" ]; then
  WS2=$RUN_DIR/ws-twin; mkdir -p "$WS2"
  tx init2 60 --root "$WS2" init
  tx ingest2 "$TIMEOUT" --root "$WS2" ingest "$CORPUS"
  tx claims2 600 --root "$WS2" claims --json
  if [ "$(xc claims2)" = "0" ]; then
    A=$(jq -r '.[].claim_id' "$RUN_DIR/claims.out" | sort | sha256sum | cut -d' ' -f1)
    B=$(jq -r '.[].claim_id' "$RUN_DIR/claims2.out" | sort | sha256sum | cut -d' ' -f1)
    [ "$A" = "$B" ] && DETERMINISM="identical" || DETERMINISM="DIVERGED"
  else
    DETERMINISM="twin_ingest_failed"
  fi
fi

CORPUS_DIGEST=$(jq -r '.corpus_digest // empty' "$CORPUS/manifest.json" 2>/dev/null || true)
STORE_BYTES=$(du -sb "$WS/.texo/store" 2>/dev/null | cut -f1)
CACHE_BYTES=$(du -sb "$WS/.texo/cache" 2>/dev/null | cut -f1)
GIT_SHA=$(git -C /home/heyoub/Code/texo rev-parse --short HEAD 2>/dev/null)

jq -n \
  --arg rung "$RUNG" --arg label "$LABEL" --arg git "$GIT_SHA" \
  --arg bin_sha "$(sha256sum "$BIN" | cut -d' ' -f1)" \
  --arg corpus "$CORPUS" --arg corpus_digest "${CORPUS_DIGEST:-}" \
  --arg ingest_wall "$(wall ingest)" --arg ingest_rss_kb "$(rss ingest)" \
  --arg verify_wall "$(wall verify)" --arg reingest_wall "$(wall reingest)" \
  --argjson exits "{\"init\":$(xc init),\"ingest\":$(xc ingest),\"verify\":$(xc verify),\"reingest\":$(xc reingest),\"claims\":$(xc claims)}" \
  --argjson yield "$YIELD" --arg determinism "$DETERMINISM" \
  --arg store_bytes "${STORE_BYTES:-0}" --arg cache_bytes "${CACHE_BYTES:-0}" --arg timeout_s "$TIMEOUT" \
  '{rung:$rung,label:$label,git:$git,binary_sha256:$bin_sha,
    corpus:$corpus,corpus_digest:$corpus_digest,timeout_s:$timeout_s,
    exits:$exits, ingest:{wall:$ingest_wall,rss_kb:$ingest_rss_kb},
    verify_wall:$verify_wall, noop_reingest_wall:$reingest_wall,
    store_bytes:$store_bytes, cache_bytes:$cache_bytes, yield:$yield, determinism:$determinism}' \
  > "$RUN_DIR/metrics.json"

echo "== $RUNG ($LABEL) -> $RUN_DIR"
cat "$RUN_DIR/metrics.json"
