#!/usr/bin/env bash
set -euo pipefail
[[ ${SWAYSOCK:-} == "$XDG_RUNTIME_DIR"/sway-ipc.*.sock && ${2:-} == bench ]]
state="$XDG_RUNTIME_DIR/native-tube-headless"
case "$1" in
  run)
    shift 2
    NT_APP_ID=nttest-bench NT_MPV_OPTS=${NT_MPV_OPTS:-ao=null} setsid "$@" > "$state/bench.log" 2>&1 < /dev/null &
    echo "$!"
    ;;
  shot)
    exec grim -o HEADLESS-1 "$3"
    ;;
  *)
    exit 2
    ;;
esac
