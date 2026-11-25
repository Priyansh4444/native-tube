#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
mode=${1:?smoke, batch, live, explore, perf or fair}
shift
case "$mode" in
    live) network=pasta:--no-map-gw ;;
    smoke|batch|explore|perf|fair) network=none ;;
    *) exit 2 ;;
esac
render=${NT_ISO_RENDER:-intel}
devices=()
case "$render" in
    intel)
        [[ $(</sys/class/drm/renderD128/device/vendor) == 0x8086 ]]
        devices+=(--device /dev/dri/renderD128)
        ;;
    software) ;;
    *) exit 2 ;;
esac

out="$PWD/.cache/iso-test/out"
mkdir -p "$out"
run_dir=$(mktemp -d "$out/$mode-XXXXXXXX")
mounts=(--volume "$out:/out:rw")
case "$mode" in
    smoke|batch|live) mounts+=(--volume "$PWD/tools/iso-test/batch.py:/iso/batch.py:ro") ;;
esac
for app in iced gpui rune; do
    var="NT_ISO_BIN_${app^^}"
    binary=$(realpath "${!var:-.cache/media/target/NOW/$app/release/nt-$app}")
    [[ -x "$binary" ]]
    mounts+=(--volume "$binary:/inputs/nt-$app:ro")
    if [[ "$mode" == perf ]]; then
        baseline=2caf0fa25b4c1973f23f5f5928fdc064eac945cb
        baseline_bin=$(realpath "${NT_ISO_BASELINE_DIR:-.cache/perf/target/$baseline}/$app/release/nt-$app")
        [[ -x "$baseline_bin" ]]
        mounts+=(--volume "$baseline_bin:/inputs/baseline-$app:ro")
    fi
done
if [[ "$mode" != live ]]; then
    fixtures=$(realpath "${NT_ISO_FIXTURES:-.cache/fixtures}")
    clip=$(realpath "${NT_ISO_CLIP:-.cache/test-clip-720p30.mp4}")
    [[ -d "$fixtures" && -f "$clip" ]]
    mounts+=(--volume "$fixtures:/fixtures:ro" --volume "$clip:/clip.mp4:ro")
fi

security=()
if [[ "$mode" == fair ]]; then
    # perf stat counts the apps' own user-space instructions. The default seccomp profile
    # allows perf_event_open only with CAP_PERFMON or CAP_SYS_ADMIN; this copy allows the call
    # without adding any capability, so the kernel's perf_event_paranoid still applies.
    python - /usr/share/containers/seccomp.json "$run_dir/seccomp.json" <<'PY'
import json, sys
profile = json.load(open(sys.argv[1]))
profile["syscalls"] = [rule for rule in profile["syscalls"]
                       if not (rule["names"] == ["perf_event_open"] and rule["action"] == "SCMP_ACT_ERRNO")]
profile["syscalls"].append({"names": ["perf_event_open"], "action": "SCMP_ACT_ALLOW", "args": []})
json.dump(profile, open(sys.argv[2], "w"))
PY
    security+=(--security-opt "seccomp=$run_dir/seccomp.json")
    mounts+=(--volume "$PWD/tools/iso-test/fair.py:/iso/fair.py:ro")
fi

printf '%s\n' "$run_dir"
monitor=
if [[ ${NT_ISO_SAMPLE_HOST:-0} == 1 ]]; then
    python tools/iso-test/host-sample.py before > "$run_dir/host.jsonl"
    python tools/iso-test/host-sample.py during >> "$run_dir/host.jsonl" &
    monitor=$!
fi
cleanup() {
    if [[ -n "$monitor" ]]; then
        kill "$monitor" 2>/dev/null || true
        wait "$monitor" 2>/dev/null || true
        python tools/iso-test/host-sample.py after >> "$run_dir/host.jsonl"
    fi
}
trap cleanup EXIT

nice -n 19 ionice -c3 podman run --rm --network "$network" --userns=keep-id \
    --security-opt no-new-privileges --cap-drop=ALL --cpus 4 --memory 6g --pids-limit 512 \
    --read-only --tmpfs /tmp:rw,nodev,nosuid,size=512m --http-proxy=false \
    --env "NT_ISO_OUT=/out/${run_dir##*/}" --env "NT_ISO_RENDER=$render" \
    --env "NT_ISO_LIVE_VIDEO_ID=${NT_ISO_LIVE_VIDEO_ID:-5C_HPTJg5ek}" \
    "${security[@]}" "${devices[@]}" "${mounts[@]}" nt-iso "$mode" "$@" 2>&1 | tee "$run_dir/driver.log"
