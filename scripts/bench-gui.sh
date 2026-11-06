#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Runs the desktop app's bench (Linux, needs xvfb-run) against a data directory,
# adds the app's idle memory, and checks the report against the budget.
# Usage: scripts/bench-gui.sh <data dir> <report.json> [familiar binary]
set -euo pipefail
data=$1 out=$2 bin=${3:-target/release/familiar}
rm -f "$out"
# A fresh machine's first launch builds font and WebKit caches and reads the
# store from disk (33 s on a CI runner). Launch once to warm those, so cold
# start measures the app, as on a machine that has run it before.
FAMILIAR_DATA_DIR=$data FAMILIAR_CONFIG=/dev/null FAMILIAR_BENCH_OUT=$out.warmup \
  timeout 120 xvfb-run -a -s "-screen 0 1280x800x24" "$bin" >/dev/null 2>&1 || true
rm -f "$out.warmup"
FAMILIAR_DATA_DIR=$data FAMILIAR_CONFIG=/dev/null FAMILIAR_BENCH_OUT=$out FAMILIAR_BENCH_HOLD=1 \
  xvfb-run -a -s "-screen 0 1280x800x24" "$bin" &
runner=$!
for _ in $(seq 240); do [ -s "$out" ] && break; sleep 0.5; done
if [ ! -s "$out" ]; then echo "no bench report after 120 s"; kill "$runner"; exit 1; fi

# Idle memory: settle, then sum anonymous memory over the app and its WebKit
# processes. That is the private memory the target means (Windows' private
# bytes): it leaves out shared libraries and the memory-mapped store.
sleep 10
app=$(pgrep -n -x familiar)
tree() { echo "$1"; for c in $(pgrep -P "$1"); do tree "$c"; done; }
kb=0
for p in $(tree "$app"); do
  v=$(awk '/^Anonymous:/ {print $2}' "/proc/$p/smaps_rollup" 2>/dev/null || echo 0)
  kb=$((kb + ${v:-0}))
done
kill "$app" 2>/dev/null || true
wait "$runner" 2>/dev/null || true
node -e 'const f=process.argv[1],r=JSON.parse(require("fs").readFileSync(f));r.idle_private_mb=+process.argv[2]/1024;require("fs").writeFileSync(f,JSON.stringify(r,null,2)+"\n")' "$out" "$kb"
node scripts/check-bench.mjs "$out"
