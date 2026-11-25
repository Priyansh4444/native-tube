#!/usr/bin/env bash
set -euo pipefail
cd /opt/native-tube
runtime=$(mktemp -d)
export XDG_RUNTIME_DIR=$runtime XDG_SESSION_TYPE=wayland
export WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe
unset DISPLAY
sway -c tools/fly-bench/sway.conf >&2 &
compositor=$!
cleanup() {
    kill "$compositor" 2>/dev/null || true
    wait "$compositor" 2>/dev/null || true
    rm -rf "$runtime"
}
trap cleanup EXIT
shopt -s nullglob
for ((i = 0; i < 100; i++)); do
    sockets=("$runtime"/sway-ipc.*.sock)
    displays=("$runtime"/wayland-*)
    if ((${#sockets[@]} == 1)) && [[ -S "${displays[0]:-}" ]]; then
        export WAYLAND_DISPLAY="${displays[0]##*/}"
        SWAYSOCK="${sockets[0]}" swaymsg -t get_outputs >&2
        node "$@"
        exit
    fi
    kill -0 "$compositor"
    sleep 0.05
done
exit 1
