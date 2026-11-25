#!/usr/bin/env bash
set -euo pipefail
mode=${1:?smoke, batch, live, explore, perf or fair}
shift
out=${NT_ISO_OUT:-/out}
mkdir -p "$out"
runtime=$(mktemp -d)
export XDG_RUNTIME_DIR="$runtime"
export HOME="$runtime/home" XDG_CONFIG_HOME="$runtime/config" XDG_DATA_HOME="$runtime/data" XDG_CACHE_HOME="$runtime/cache"
mkdir -p "$HOME"
unset DISPLAY WAYLAND_DISPLAY WAYLAND_SOCKET SWAYSOCK HYPRLAND_INSTANCE_SIGNATURE DBUS_SESSION_BUS_ADDRESS
export XDG_SESSION_TYPE=wayland WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1
export WLR_RENDERER=gles2 WLR_HEADLESS_OUTPUTS=1
export NT_INPUT_BACKEND=sway NT_PROBE=1 NT_MUTE=1 NT_NO_MPRIS=1 NT_TRACE=1 NT_MPV_OPTS=ao=null
export PYTHONDONTWRITEBYTECODE=1
case "${NT_ISO_RENDER:-intel}" in
  intel)
    [[ -c /dev/dri/renderD128 && $(</sys/class/drm/renderD128/device/vendor) == 0x8086 ]]
    shopt -s nullglob
    devices=(/dev/dri/*)
    [[ ${#devices[@]} == 1 && ${devices[0]} == /dev/dri/renderD128 ]]
    export WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128
    export VK_DRIVER_FILES=/usr/share/vulkan/icd.d/intel_icd.json
    ;;
  software)
    export WLR_RENDERER_ALLOW_SOFTWARE=1 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe
    export VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json
    # Without a DRM FD, wlroots cannot start its GLES2 renderer. The clients still use Mesa.
    export WLR_RENDERER=pixman
    ;;
  *) exit 2 ;;
esac
case "$mode" in
  live) export NT_LIVE=1 ;;
  smoke|batch|explore|perf|fair)
    export NT_LIVE=0 NT_FIXTURE_MODE=replay NT_FIXTURES=/fixtures NT_VIDEO_OVERRIDE=/clip.mp4
    ;;
  *) exit 2 ;;
esac
sway -d -c /iso/sway.conf > "$out/sway.log" 2>&1 &
compositor=$!
pointer=
cleanup() {
    if [[ -n "$pointer" ]]; then
        kill "$pointer" 2>/dev/null || true
        wait "$pointer" 2>/dev/null || true
    fi
    kill "$compositor" 2>/dev/null || true
    wait "$compositor" 2>/dev/null || true
}
trap cleanup EXIT
shopt -s nullglob
for ((i = 0; i < 200; i++)); do
    sockets=("$runtime"/sway-ipc.*.sock)
    displays=("$runtime"/wayland-*)
    if ((${#sockets[@]} == 1)) && [[ -S "${displays[0]:-}" ]]; then
        export SWAYSOCK="${sockets[0]}" WAYLAND_DISPLAY="${displays[0]##*/}"
        break
    fi
    kill -0 "$compositor"
    sleep 0.05
done
: "${SWAYSOCK:?private Sway socket did not start}"
[[ ! -e "$XDG_RUNTIME_DIR/nt-input.paused" && ! -e "$HOME/.local/state/nt-input.paused" ]]
nt-input __anchor > "$out/virtual-pointer.log" 2>&1 &
pointer=$!
for ((i = 0; i < 100; i++)); do
    if [[ -s "$out/virtual-pointer.log" ]]; then break; fi
    kill -0 "$pointer"
    sleep 0.05
done
grep -qx 'private pointer ready' "$out/virtual-pointer.log"
swaymsg -t get_outputs > "$out/outputs.json"
swaymsg -t get_seats > "$out/seats.json"
python - "$out" <<'PY'
import json, pathlib, sys
out = pathlib.Path(sys.argv[1])
outputs = [o for o in json.loads((out / "outputs.json").read_text()) if o["active"]]
assert len(outputs) == 1, outputs
screen = outputs[0]
assert screen["name"] == "HEADLESS-1" and screen["scale"] == 1, screen
assert screen["rect"] == dict(x=0, y=0, width=1280, height=800), screen
seats = json.loads((out / "seats.json").read_text())
assert [seat["name"] for seat in seats] == ["seat0"], seats
PY
{
    printf 'Mode %s\nRuntime %s\nSway socket %s\nWayland %s\n' "$mode" "$XDG_RUNTIME_DIR" "$SWAYSOCK" "$WAYLAND_DISPLAY"
    user_runtime="/run/user/$(id -u)"
    host_sockets=("$user_runtime"/wayland-* "$user_runtime"/hypr/*/.*.sock "$user_runtime"/bus)
    for socket in "${host_sockets[@]}"; do
        [[ ! -S "$socket" ]]
    done
    echo 'No Wayland, Hyprland or session bus socket in the host-standard runtime path'
    printf 'Input device nodes\n'
    ls /dev/input 2>/dev/null || true
    printf 'Render nodes\n'
    ls /dev/dri 2>/dev/null || true
    printf 'Network interfaces\n'
    cat /proc/net/dev
} > "$out/isolation.txt"
if [[ "$mode" == explore ]]; then
    export NT_EXPLORE_ROOT=/iso NT_EXPLORE_OUT="$out" NT_INPUT=/usr/local/bin/nt-input
    if (( $# == 0 )); then set -- rune iced gpui; fi
    for app in "$@"; do
        [[ "$app" == iced || "$app" == gpui || "$app" == rune ]]
        code=0
        nt-explore "/inputs/nt-$app" --seed 42 --steps 16 --minutes 0.5 > "$out/$app.log" 2>&1 || code=$?
        printf '%s %s\n' "$app" "$code" >> "$out/status.txt"
    done
elif [[ "$mode" == perf ]]; then
    python -u /iso/perf-sway.py "$out" "$@"
elif [[ "$mode" == fair ]]; then
    python -u /iso/fair.py "$out" "$@"
else
    python -u /iso/batch.py "$mode" "$out" "$@"
fi
