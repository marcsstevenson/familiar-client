#!/usr/bin/env bash
# Cold-cache timings for WP 0.5 (Linux, needs root to drop the page cache).
# Each run drops the OS page cache, then times one fresh process end to end.
#   tools/store-bench/cold.sh DATA_DIR FOLDER_ID QUERY [RUNS]
set -euo pipefail
data=$1; folder=$2; query=$3; runs=${4:-10}
bin=$(dirname "$0")/../target/release/store-bench
ms() { echo $(( ($2 - $1) / 1000000 )); }
run() {
  local label=$1; shift
  local times=()
  for _ in $(seq "$runs"); do
    sync; echo 3 > /proc/sys/vm/drop_caches
    local t0; t0=$(date +%s%N)
    "$@" > /dev/null
    times+=("$(ms "$t0" "$(date +%s%N)")")
  done
  printf '%s\t%s\n' "$label" "$(printf '%s\n' "${times[@]}" | sort -n | tr '\n' ' ')"
}
run "cold list --json (ms, sorted)" "$bin" list --data "$data" --folder "$folder" --json
run "cold search '$query' (ms, sorted)" "$bin" search --data "$data" "$query"
