#!/usr/bin/env bash
# Run the benchmark scenario in one app, alone, and wait for its report.
#   bench/run-scenario.sh <iced|gpui|rune> [out-dir]
set -euo pipefail
fw=$1; root=$(cd "$(dirname "$0")/.." && pwd); out=${2:-$root/bench/results/$fw}
mkdir -p "$out"; rm -f "$out/report.json" "$out"/*.png
# Benchmarks answer only from recorded data: a missing recording is an error, not a live request.
NT_FIXTURE_MODE="${NT_FIXTURE_MODE:-replay}" NT_AUTOPILOT="$root/bench/scenario.json" NT_REPORT="$out/report.json" \
NT_SNAPSHOT_OUTPUT="NT-${fw^^}" NT_SNAPSHOT_DIR="$out" NT_DATA_DIR="$out/data" \
  "$root/tools/headless.sh" run "$fw" "$root/apps/$fw/target/release/nt-$fw" >/dev/null
for _ in $(seq 240); do [[ -s "$out/report.json" ]] && break; sleep 1; done
sleep 2; "$root/tools/headless.sh" kill "$fw"
[[ -s "$out/report.json" ]] || { echo "$fw: no report"; "$root/tools/headless.sh" log "$fw" | tail -5; exit 1; }
jq -r --arg fw "$fw" '"\($fw): failed_waits=\(.failed_waits) rss_peak=\(.rss_mb_peak|floor)MB marks=\(.marks_ms|to_entries|map("\(.key)=\(.value|floor)")|join(" "))"' "$out/report.json"
