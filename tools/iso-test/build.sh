#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
context=$(mktemp -d)
trap 'rm -rf -- "$context"' EXIT
mkdir -p "$context/tools/"{iso-test,performance} "$context/bench"
for tool in nt-input nt-explore; do
    mkdir -p "$context/tools/$tool"
    cp -r "tools/$tool"/{Cargo.toml,Cargo.lock,src} "$context/tools/$tool/"
done
cp -r tools/nt-explore/tests "$context/tools/nt-explore/"
cp tools/iso-test/{Dockerfile,sway.patch,start.sh,batch.py,sway.conf,explore-headless.sh,perf-sway.py,fair.py} "$context/tools/iso-test/"
cp tools/{check-performance.py,perf-budget.json} "$context/tools/"
cp tools/performance/*.py "$context/tools/performance/"
cp bench/shells.py "$context/bench/"
nice -n 19 ionice -c3 podman build --jobs 1 --cpu-period 100000 --cpu-quota 400000 --memory 6g \
    --build-arg "BENCH_IMAGE=${NT_ISO_BENCH_IMAGE:-nt-fly-bench:local}" \
    -t nt-iso -f "$context/tools/iso-test/Dockerfile" "$context"
