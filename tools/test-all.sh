#!/usr/bin/env bash
# Run all three property-test levels and print a summary table; exits non-zero on any failure.
#
#   tools/test-all.sh [--baseline <git revision>]
#
# 1. ./check.sh, then core's cargo test (unit tests and proptest properties);
# 2. Bombadil on the ntrepl terminal REPL for NT_BOMBADIL_SECONDS (default 180), in sessions;
# 3. nt-explore on each existing release app binary on NT-BENCH, one app at a time, for
#    NT_EXPLORE_MINUTES (default 2, the most a GUI slice may take).
# With --baseline, also tools/check-performance.sh against that revision.
#
# The machine stays usable: every stage runs at the lowest CPU and I/O priority with at most
# CARGO_BUILD_JOBS=4 compiler jobs, and dies with this script (so killing it never leaves an
# explorer driving input). Existing app binaries are used as they are; nothing rebuilds them.
set -uo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root" || exit 1
export CARGO_BUILD_JOBS=4

export NT_FIXTURES="${NT_FIXTURES:-$root/.cache/fixtures}"
export NT_FIXTURE_MODE=replay NT_LIVE=0 NT_MUTE=1 NT_NO_MPRIS=1

baseline=${NT_PERF_BASELINE:-}
if (($#)); then
  if [[ "$1" != --baseline || $# != 2 ]]; then
    echo "Usage: $0 [--baseline <git revision>]" >&2
    exit 2
  fi
  baseline=$2
fi

terminal_seconds=${NT_BOMBADIL_SECONDS:-180}
native_minutes=${NT_EXPLORE_MINUTES:-2}
seed_args=()
if [[ -n "${NT_EXPLORE_SEED:-}" ]]; then seed_args=(--seed "$NT_EXPLORE_SEED"); fi
mkdir -p tests/bombadil/out || exit 1
logs=$(mktemp -d "$root/tests/bombadil/out/all-XXXXXXXX") || exit 1

labels=()
statuses=()
outputs=()
failed=0
record() {
  labels+=("$1")
  statuses+=("$2")
  outputs+=("$3")
  if [[ "$2" == FAIL* ]]; then failed=1; fi
}

current=
# On Ctrl-C or kill, stop the running stage (an explorer stops its app; nt-input's watchdog
# gives the cursor and focus back) and report what ran.
trap 'if [[ -n "$current" ]]; then kill -TERM -- "-$current" 2>/dev/null || kill -TERM "$current" 2>/dev/null; wait "$current"; fi; echo "test-all: interrupted" >&2; exit 130' INT TERM HUP

stage() {
  local label=$1 key=$2 log rc
  shift 2
  log="$logs/$key.log"
  printf '== %s\n' "$label"
  # Shell functions run in a child bash (they and the variables they use are exported).
  if declare -F "$1" >/dev/null; then set -- bash -c '"$@"' stage "$@"; fi
  local explorer="$root/tools/nt-explore/target/debug/nt-explore"
  if [[ "$key" == native-* || ( "$key" == performance && -x "$explorer" ) ]]; then
    set -- "$explorer" --owned-exec "$$" "$@"
  fi
  # Run in the background and wait, so a signal reaches the trap below at once.
  nice -n 19 ionice -c3 setsid setpriv --pdeathsig TERM -- "$@" >"$log" 2>&1 &
  current=$!
  wait "$current"
  rc=$?
  current=
  if ((rc == 0)); then
    record "$label" PASS "$log"
  elif ((rc == 3)) && [[ "$key" == native-* ]]; then
    record "$label" 'SKIP (no NT_STATE)' "$log"
  elif ((rc == 4)) && [[ "$key" == native-* ]]; then
    record "$label" 'SKIP (desktop busy)' "$log"
  else
    record "$label" "FAIL ($rc)" "$log"
    tail -n 12 "$log"
  fi
  return "$rc"
}

explorer_checks() {
  local manifest=tools/nt-explore/Cargo.toml
  cargo fmt --manifest-path "$manifest" --check &&
    cargo clippy --locked --manifest-path "$manifest" --all-targets --all-features -- -D warnings
}

terminal_test() {
  cargo build --locked --offline --manifest-path core/Cargo.toml --bin ntrepl || return
  if [[ ! -x tests/bombadil/node_modules/.bin/bombadil ]]; then
    npm ci --prefix tests/bombadil --no-audit --no-fund || return
  fi
  tests/bombadil/run.sh explore "$terminal_seconds" "$logs/terminal"
}

explorer_ready() {
  local manifest=tools/nt-explore/Cargo.toml
  cargo test --locked --offline --manifest-path "$manifest" &&
    cargo build --locked --offline --manifest-path "$manifest" &&
    cargo build --locked --offline --release --manifest-path tools/nt-input/Cargo.toml
}

export -f explorer_checks terminal_test explorer_ready
export terminal_seconds logs

stage 'Workspace check' check ./check.sh
stage 'Explorer check' explorer-check explorer_checks
stage 'Core properties' core cargo test --locked --offline --manifest-path core/Cargo.toml
stage 'Bombadil terminal' bombadil terminal_test
stage 'Explorer unit tests and build' explorer-ready explorer_ready
explorer_status=$?

for app in iced gpui rune; do
  binary="apps/$app/target/release/nt-$app"
  if [[ ! -x "$binary" ]]; then
    record "Native $app" 'SKIP (no binary)' "$binary"
  elif ((explorer_status != 0)); then
    record "Native $app" 'SKIP (build failed)' "$logs/explorer-ready.log"
  else
    stage "Native $app" "native-$app" tools/nt-explore/target/debug/nt-explore "$binary" \
      --minutes "$native_minutes" "${seed_args[@]}"
    # A breather between GUI slices for whoever is using the desktop.
    sleep 3
  fi
done

if [[ -n "$baseline" ]]; then
  stage 'Performance budget' performance tools/check-performance.sh "$baseline"
fi

printf '\n%-31s %-21s %s\n' 'Stage' 'Result' 'Output'
for i in "${!labels[@]}"; do
  printf '%-31s %-21s %s\n' "${labels[i]}" "${statuses[i]}" "${outputs[i]}"
done
exit "$failed"
