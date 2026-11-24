#!/usr/bin/env bash
# Build one milestone at a time into media-owned Cargo targets.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$PWD
media=$root/.cache/media
mkdir -p "$media/worktrees" "$media/target" "$media/out/build"
exec 9>"$media/build.lock"
flock -n 9
export CARGO_BUILD_JOBS=4
failed=0
apps=(iced gpui rune)
case "${1:-}" in
    --core) apps=(core); shift ;;
    --with-core) apps+=(core); shift ;;
esac
if (( $# == 0 )); then
    set -- 96df4ec cb25193 bf276b1 2caf0fa NOW 96df4ec-routed
fi

for rev in "$@"; do
    case "$rev" in
        96df4ec|cb25193|bf276b1|2caf0fa) base=$rev ;;
        96df4ec-routed) base=96df4ec ;;
        NOW) base=HEAD ;;
        *) echo "Unknown milestone: $rev" >&2; exit 2 ;;
    esac
    tree="$media/worktrees/$rev"
    git worktree add --detach "$tree" "$base"
    trap 'git worktree remove --force "$tree"' EXIT
    if [[ "$rev" == 96df4ec-routed ]]; then
        # Only the three official app-ID changes needed by the headless test rule.
        git show --format= 66f442f -- apps/{iced,gpui,rune}/src/main.rs > "$media/out/build/96df4ec-routing.diff"
        git -C "$tree" apply "$media/out/build/96df4ec-routing.diff"
    fi
    if [[ "$rev" == NOW ]]; then
        git diff --name-only -z HEAD -- apps core assets > "$media/now-paths"
        git ls-files -z --others --exclude-standard -- apps core assets >> "$media/now-paths"
        while IFS= read -r -d '' path; do
            [[ "$path" == */shots/* ]] && continue
            if [[ -e "$path" ]]; then
                cp -p --parents "$path" "$tree"
            else
                rm -f "$tree/$path"
            fi
        done < "$media/now-paths"
        # The three builds use this source snapshot even if the main tree is edited later.
        (cd "$tree"; git ls-files -z --cached --others --exclude-standard -- apps core assets |
            while IFS= read -r -d '' path; do
                if [[ -f "$path" ]]; then printf '%s\0' "$path"; fi
            done |
            sort -zu | xargs -0 -r sha256sum) > "$media/out/build/NOW-source.sha256"
    fi
    for app in "${apps[@]}"; do
        printf '%s %s\n' "$rev" "$app"
        target="$media/target/$rev/$app"
        manifest="$tree/apps/$app/Cargo.toml"
        binary="nt-$app"
        if [[ "$app" == core ]]; then
            manifest="$tree/core/Cargo.toml"
            binary=ntctl
        fi
        if [[ ! -d "$target" ]]; then
            seed="$root/.cache/perf/target/$(git rev-parse 2caf0fa)/$app"
            [[ "$rev" == 96df4ec-routed ]] && seed="$media/target/96df4ec/$app"
            [[ "$rev" == NOW ]] && seed="$root/apps/$app/target"
            [[ "$app" == core ]] && seed="$root/core/target"
            if [[ -d "$seed/release" ]]; then
                mkdir -p "$target"
                nice -n 19 ionice -c3 cp -a --reflink=auto "$seed/release" "$target/"
                # A seeded executable is never evidence of a completed build.
                rm -f "$target/release/$binary"
            fi
        fi
        nice -n 19 ionice -c3 cargo build --locked --release \
            --manifest-path "$manifest" --bin "$binary" --target-dir "$target" \
            > "$media/out/build/$rev-$app.log" 2>&1 || {
                tail -n 20 "$media/out/build/$rev-$app.log" >&2
                failed=1
                continue
            }
        sha256sum "$target/release/$binary"
    done
    git worktree remove --force "$tree"
    trap - EXIT
done
exit "$failed"
