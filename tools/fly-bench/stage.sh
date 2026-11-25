#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
context=.cache/fly-bench/context
mkdir -p "$context"/{bin,bench,tools,.cache/media}
for app in iced gpui rune; do
    cp --reflink=auto ".cache/media/target/NOW/$app/release/nt-$app" "$context/bin/"
done
cp --reflink=auto .cache/media/target/NOW/core/release/ntctl "$context/bin/"
cp --reflink=auto -a .cache/media/{fixtures,tour-fixtures-now} "$context/.cache/media/"
cp --reflink=auto .cache/test-clip-720p30.mp4 "$context/.cache/"
mkdir -p "$context/tools/"{fly-bench,media,performance}
cp tools/fly-bench/{bench.py,run.sh,sway.conf,eval.mjs} "$context/tools/fly-bench/"
cp tools/media/{collect,metrics,recorder,resources}.py tools/media/{measure,tour}.json "$context/tools/media/"
cp tools/performance/{analysis,procfs,stories}.py "$context/tools/performance/"
cp bench/{cdp.mjs,shells.py} "$context/bench/"
sha256sum -c --quiet .cache/media/out/build/NOW-source.sha256
sha256sum "$context"/bin/* "$context"/.cache/test-clip-720p30.mp4 "$context"/.cache/media/*/index.txt
