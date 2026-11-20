#!/usr/bin/env bash
# Measure the existing release builds in a live journey or compare them with a detached baseline.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
export CARGO_BUILD_JOBS=4
mode=quick
if [[ "${1:-}" == --live ]]; then
  shift
  if [[ ! -x "$root/tools/nt-explore/target/debug/nt-explore" ]]; then
    nice -n 19 ionice -c3 cargo build --locked --offline --manifest-path tools/nt-explore/Cargo.toml
  fi
  mkdir -p "$root/.cache/perf/out"
  out=$(mktemp -d "$root/.cache/perf/out/live-XXXXXXXX")
  echo "Live YouTube performance outputs: $out"
  export PYTHONDONTWRITEBYTECODE=1
  exec nice -n 19 ionice -c3 python3 "$root/tools/check-performance.py" \
    --mode live live - "$out" "$@"
fi
if [[ "${1:-}" == --soak ]]; then
  mode=soak
  shift
fi
if (($# < 1)); then
  echo "Usage: $0 [--soak] <baseline git revision> [iced|gpui|rune ...]" >&2
  echo "       $0 --live [--height PIXELS] [iced|gpui|rune ...]" >&2
  exit 2
fi
baseline=$(git rev-parse --verify --end-of-options "$1^{commit}")
shift
if (($# == 0)); then set -- iced gpui rune; fi
apps=("$@")
for app in "${apps[@]}"; do
  case "$app" in iced|gpui|rune) ;; *) echo "Unknown app: $app" >&2; exit 2 ;; esac
  if [[ ! -x "$root/apps/$app/target/release/nt-$app" ]]; then
    echo "Missing candidate release binary for $app" >&2
    exit 1
  fi
done

cache="$root/.cache/perf"
mkdir -p "$cache" "$cache/out"
out=$(mktemp -d "$cache/out/compare-XXXXXXXX")
work=$(mktemp -d "$cache/worktree-XXXXXXXX")
scratch=$(mktemp -d "$cache/binaries-XXXXXXXX")
cleanup() {
  git worktree remove "$work" >/dev/null 2>&1 || true
  rmdir "$work" 2>/dev/null || true
  rm -rf -- "$scratch"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

echo "Performance outputs: $out"
for app in "${apps[@]}"; do
  cp --reflink=auto "$root/apps/$app/target/release/nt-$app" "$scratch/candidate-$app"
done
nice -n 19 ionice -c3 cargo build --locked --offline --manifest-path tools/nt-explore/Cargo.toml
nice -n 19 ionice -c3 git worktree add --detach "$work" "$baseline"
for app in "${apps[@]}"; do
  echo "Building baseline $app (${baseline:0:12})"
  target="$cache/target/$baseline/$app"
  # One app at a time, at the lowest priority, so the desktop stays usable.
  if ! CARGO_TARGET_DIR="$target" nice -n 19 ionice -c3 cargo build --release --locked \
      --jobs 4 --manifest-path "$work/apps/$app/Cargo.toml" \
      --bin "nt-$app" >"$out/build-$app.log" 2>&1; then
    tail -n 30 "$out/build-$app.log" >&2
    exit 1
  fi
  cp --reflink=auto "$target/release/nt-$app" "$scratch/baseline-$app"
done

git worktree remove "$work"
export PYTHONDONTWRITEBYTECODE=1
nice -n 19 ionice -c3 python3 "$root/tools/check-performance.py" --mode "$mode" \
  "$baseline" "$scratch" "$out" "${apps[@]}"
