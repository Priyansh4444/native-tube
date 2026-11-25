#!/usr/bin/env bash
set -euo pipefail
cd /opt/native-tube
out=${1:-/tmp/nt-results}
shift
mkdir -p "$out"
runtime=$(mktemp -d)
export XDG_RUNTIME_DIR=$runtime
export WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe
export VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json
export XDG_SESSION_TYPE=wayland
unset DISPLAY
sway -c tools/fly-bench/sway.conf > "$out/sway.log" 2>&1 &
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
        export SWAYSOCK="${sockets[0]}" WAYLAND_DISPLAY="${displays[0]##*/}"
        break
    fi
    kill -0 "$compositor"
    sleep 0.05
done
swaymsg -t get_outputs > "$out/outputs.json"
{
    uname -a
    grep -m1 'model name' /proc/cpuinfo
    nproc
    head -n 3 /proc/meminfo
    pacman -Q glibc mesa vulkan-swrast sway mpv chromium ffmpeg
    vulkaninfo --summary 2>&1 || true
    eglinfo -B 2>&1 || true
} > "$out/vm.txt"
python tools/fly-bench/bench.py "$out" "$@"
