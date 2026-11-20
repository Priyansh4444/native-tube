#!/usr/bin/env bash
# Bombadil on the ntrepl terminal REPL (fixture replay, no network).
#
#   tests/bombadil/run.sh test [Bombadil options]        one session; it ends at `quit` or --time-limit
#   tests/bombadil/run.sh test --reproduce DIR [opts]    replay a session's trace.jsonl
#   tests/bombadil/run.sh shrink --trace DIR [opts]      shrink a violating session
#   tests/bombadil/run.sh explore SECONDS OUT_DIR        back-to-back sessions for SECONDS, each in
#                                                        OUT_DIR/session-N; stops at a violation
#
# The terminal size and timing options are fixed here, because reproduce and shrink must use the
# same ones as the run that found a violation. NT_REPL_BIN overrides the REPL binary.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
export NT_FIXTURES="${NT_FIXTURES:-$root/.cache/fixtures}"
export NT_FIXTURE_MODE=replay NT_MUTE=1 NT_NO_MPRIS=1
repl=${NT_REPL_BIN:-$root/core/target/debug/ntrepl}

bombadil() {
  local mode=$1
  shift
  "$here/node_modules/.bin/bombadil" terminal "$mode" \
    --specification "$here/ntrepl.spec.ts" --columns 100 --rows 24 --scrollback-lines-max 8 \
    --quiescence-timeout-ms 40 "$@" -- "$repl"
}

mode=${1:-test}
if (($#)); then shift; fi
case "$mode" in
  test | shrink) bombadil "$mode" "$@" ;;
  explore)
    seconds=${1:?seconds}
    out=${2:?output directory}
    if [[ ! "$seconds" =~ ^[1-9][0-9]{0,5}$ ]] || ((seconds < 5)); then
      echo 'explore duration must be at least five whole seconds' >&2
      exit 2
    fi
    mkdir -p "$out"
    end=$((SECONDS + seconds))
    session=0
    while ((end - SECONDS >= 5)); do
      session=$((session + 1))
      dir="$out/session-$session"
      echo "== session $session ($((end - SECONDS)) s left): $dir"
      if ! bombadil test --exit-on-violation --time-limit "$((end - SECONDS))s" \
        --output-path "$dir" >"$dir.log" 2>&1; then
        tail -n 40 "$dir.log"
        echo "violation in session $session; reproduce: $0 test --reproduce $dir; shrink: $0 shrink --trace $dir"
        exit 2
      fi
      tail -n 3 "$dir.log"
    done
    echo "$session sessions, no violations"
    ;;
  *)
    sed -n '2,10p' "$0" >&2
    exit 2
    ;;
esac
