#!/usr/bin/env bash
# Off-screen test displays for the NativeTube apps, on the running Hyprland session.
#
#   tools/headless.sh setup                  create the outputs and rules (idempotent)
#   tools/headless.sh run <fw> <cmd...>      run an app on its output in the background, print its PID
#   tools/headless.sh shot <fw> <out.png>    screenshot that output
#   tools/headless.sh kill <fw>              stop the app started by `run`
#   tools/headless.sh teardown               remove the outputs
#
# <fw> is iced, gpui or rune. Each gets its own 1280x800 @60 headless output far
# to the right of the real screens, its own workspace, and a window rule that
# sends windows whose Wayland app_id is "nttest-<fw>" there without focusing them,
# so a test run never moves the user's focus or covers their screen.
# `run` sets NT_APP_ID=nttest-<fw>, which the apps use as their app_id; launched normally they
# are "nt-<fw>" and open on the real screen like any app.
set -euo pipefail

declare -A POS=([iced]=10000 [gpui]=12000 [rune]=14000 [bench]=16000)
STATE="${XDG_RUNTIME_DIR:-/tmp}/native-tube-headless"
mkdir -p "$STATE"

out_name() { echo "NT-${1^^}"; }

need_fw() {
  [[ -n "${POS[$1]:-}" ]] || { echo "unknown target '$1' (iced, gpui, rune, bench)" >&2; exit 2; }
}

setup_one() {
  local fw=$1 out ws pos
  out=$(out_name "$fw"); pos=${POS[$fw]}
  if ! hyprctl -j monitors | jq -e --arg o "$out" '.[] | select(.name == $o)' >/dev/null; then
    hyprctl output create headless "$out" >/dev/null
    sleep 0.3
  fi
  hyprctl eval "hl.monitor({ output = \"$out\", mode = \"1280x800@60\", position = \"${pos}x0\", scale = 1 })" >/dev/null
  # Whatever workspace Hyprland gave the output is the one its app goes to.
  ws=$(hyprctl -j monitors | jq -r --arg o "$out" '.[] | select(.name == $o) | .activeWorkspace.id')
  hyprctl eval "hl.window_rule({ match = { class = \"^nttest-$fw\$\" }, workspace = \"$ws silent\" })" >/dev/null
  hyprctl eval "hl.window_rule({ match = { class = \"^nttest-$fw\$\" }, no_initial_focus = true })" >/dev/null 2>&1 || true
}

case "${1:-}" in
  setup)
    for fw in iced gpui rune; do setup_one "$fw"; done
    hyprctl -j monitors | jq -r '.[] | select(.name | startswith("NT-")) | "\(.name) \(.width)x\(.height) at \(.x),\(.y) workspace \(.activeWorkspace.name)"'
    ;;
  run)
    fw=${2:-}; need_fw "$fw"; shift 2
    setup_one "$fw"
    log="$STATE/$fw.log"
    # Test runs are silent and stay off YouTube's servers as much as possible:
    # - muted;
    # - YouTube answers are recorded once into .cache/fixtures and replayed after that;
    # - the player plays a locally generated test clip instead of the YouTube video.
    # Set NT_LIVE=1 to talk to YouTube and play real videos (still muted).
    root=$(cd "$(dirname "$0")/.." && pwd)
    export NT_MUTE="${NT_MUTE:-1}" NT_APP_ID="nttest-$fw"
    # Test runs get their own library, never the owner's (~/.local/share/native-tube).
    export NT_DATA_DIR="${NT_DATA_DIR:-$root/.cache/test-data/$fw}"
    if [[ "${NT_LIVE:-0}" != 1 ]]; then
      export NT_FIXTURES="${NT_FIXTURES:-$root/.cache/fixtures}" NT_FIXTURE_MODE="${NT_FIXTURE_MODE:-auto}"
      export NT_VIDEO_OVERRIDE="${NT_VIDEO_OVERRIDE:-$root/.cache/test-clip-720p30.mp4}"
      echo "headless.sh: muted; YouTube data replayed from .cache/fixtures; the player plays the local test clip (set NT_LIVE=1 for real videos)" > "$log"
    else
      echo "headless.sh: muted; live YouTube data and real videos" > "$log"
    fi
    # The window rule above puts the window on the headless output, unfocused.
    setsid "$@" >> "$log" 2>&1 < /dev/null &
    echo $! > "$STATE/$fw.pid"
    cat "$STATE/$fw.pid"
    ;;
  shot)
    fw=${2:-}; need_fw "$fw"
    grim -o "$(out_name "$fw")" "${3:?output png path}"
    ;;
  kill)
    fw=${2:-}; need_fw "$fw"
    if [[ -s "$STATE/$fw.pid" ]]; then
      kill "$(cat "$STATE/$fw.pid")" 2>/dev/null || true
      rm -f "$STATE/$fw.pid"
    fi
    ;;
  log)
    fw=${2:-}; need_fw "$fw"; cat "$STATE/$fw.log" 2>/dev/null || true
    ;;
  teardown)
    for fw in iced gpui rune; do hyprctl output remove "$(out_name "$fw")" >/dev/null 2>&1 || true; done
    ;;
  *)
    sed -n '2,15p' "$0"; exit 2
    ;;
esac
