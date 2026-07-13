#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
texo=${TEXO:-"$repo_root/target/debug/texo"}
for command in git jq /usr/bin/time sha256sum rustc uname nproc; do
  command -v "$command" >/dev/null || {
    echo "required command is unavailable: $command" >&2
    exit 1
  }
done
[[ -x "$texo" ]] || {
  echo "build Texo first or set TEXO to an executable" >&2
  exit 1
}

scratch=$(mktemp -d "${TMPDIR:-/tmp}/texo-intelligence-measure.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
git clone -q --no-hardlinks "$repo_root" "$scratch/repo"
"$texo" --root "$scratch/repo" --workspace demo init >/dev/null

/usr/bin/time -f '{"wall_seconds":%e,"max_rss_kib":%M}' \
  -o "$scratch/cold-time.json" \
  "$texo" --root "$scratch/repo" --workspace demo index --json \
  >"$scratch/cold-index.json"
/usr/bin/time -f '{"wall_seconds":%e,"max_rss_kib":%M}' \
  -o "$scratch/warm-time.json" \
  "$texo" --root "$scratch/repo" --workspace demo index --json \
  >"$scratch/warm-index.json"

jq -e '.source.coverage.truncated == false and .code.coverage.truncated == false' \
  "$scratch/cold-index.json" >/dev/null
jq -e '.source.already_indexed == true and .code.already_indexed == true' \
  "$scratch/warm-index.json" >/dev/null

jq -n \
  --arg commit "$(git -C "$repo_root" rev-parse HEAD)" \
  --arg branch "$(git -C "$repo_root" branch --show-current)" \
  --arg rustc_release "$(rustc --version --verbose | awk '/^release:/ {print $2}')" \
  --arg rustc_host "$(rustc --version --verbose | awk '/^host:/ {print $2}')" \
  --arg llvm_version "$(rustc --version --verbose | awk -F': ' '/^LLVM version:/ {print $2}')" \
  --arg kernel "$(uname -srmo)" \
  --arg binary "$texo" \
  --arg binary_sha256 "$(sha256sum "$texo" | awk '{print $1}')" \
  --argjson logical_cpus "$(nproc)" \
  --argjson memory_kib "$(awk '/MemTotal/ {print $2}' /proc/meminfo)" \
  --slurpfile cold "$scratch/cold-index.json" \
  --slurpfile warm "$scratch/warm-index.json" \
  --slurpfile cold_time "$scratch/cold-time.json" \
  --slurpfile warm_time "$scratch/warm-time.json" \
  '{schema:"texo.intelligence-performance.v1",commit:$commit,branch:$branch,toolchain:{rustc_release:$rustc_release,rustc_host:$rustc_host,llvm_version:$llvm_version},host:{kernel:$kernel,logical_cpus:$logical_cpus,memory_kib:$memory_kib},binary:{path:$binary,sha256:$binary_sha256},cold:{source_files:$cold[0].source.sources_captured,source_coverage:$cold[0].source.coverage,code_format:$cold[0].code.format,code_occurrences:$cold[0].code.coverage.occurrences,code_coverage:$cold[0].code.coverage,wall_seconds:$cold_time[0].wall_seconds,max_rss_kib:$cold_time[0].max_rss_kib},warm:{source_already_indexed:$warm[0].source.already_indexed,code_already_indexed:$warm[0].code.already_indexed,wall_seconds:$warm_time[0].wall_seconds,max_rss_kib:$warm_time[0].max_rss_kib}}'
