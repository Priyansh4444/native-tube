#!/usr/bin/env bash
# The Rust gate checks formatting, Clippy, and Hawk.
#   ./check.sh          check everything
#   ./check.sh --fix    apply rustfmt first
#   NT_SKIP_HAWK=1 ./check.sh    skip the separate instrumented Hawk build
set -euo pipefail
cd "$(dirname "$0")"
# The prebuilt release uses this exact compiler, separate from rust-toolchain.toml.
HAWK_VERSION=0.1.17
HAWK_TOOLCHAIN=1.99.0
hawk_cache="$PWD/.cache/hawk"
hawk_bin="$hawk_cache/$HAWK_VERSION"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
cargo_priority=(nice -n 19)
if command -v ionice >/dev/null 2>&1; then cargo_priority+=(ionice -c3); fi

bootstrap_hawk() (
  if ! rustup run "$HAWK_TOOLCHAIN" rustc --version >/dev/null 2>&1; then
    printf 'Installing Hawk compiler Rust %s\n' "$HAWK_TOOLCHAIN"
    "${cargo_priority[@]}" rustup toolchain install "$HAWK_TOOLCHAIN" --profile minimal || return
  fi
  if [[ -x "$hawk_bin/cargo-hawk" && -x "$hawk_bin/cargo-hawk-driver" &&
        -x "$hawk_bin/cargo-hawk-rustdoc" ]]; then
    return 0
  fi

  local host digest asset work
  host=$(rustup run "$HAWK_TOOLCHAIN" rustc --print host-tuple) || return
  # SHA-256 digests from the corresponding astral-sh/hawk GitHub release assets.
  case "$host" in
    aarch64-apple-darwin)       digest=b16fa5522e9cfe3866710b37bfe3c78253a813817296c72fe804574da7be4090 ;;
    aarch64-unknown-linux-gnu)  digest=951d5885c209b9875196f2ccb92e295fdadf06c2db2b6ea07f60edf3ba8b478b ;;
    x86_64-apple-darwin)       digest=a935dcde6283581fda01164def9bc63cfb9f5fb36f6d68cfe7522f4edfff1d5c ;;
    x86_64-unknown-linux-gnu)  digest=f5a76a43aa6be1e55e44f5eac13310193ba77729673799250065f1d201018016 ;;
    *) printf 'Hawk %s has no pinned release archive for %s\n' "$HAWK_VERSION" "$host" >&2; return 1 ;;
  esac

  mkdir -p "$hawk_cache" || return
  work=$(mktemp -d "$hawk_cache/install.XXXXXXXX") || return
  trap 'rm -rf -- "$work"' EXIT
  cd "$work" || return
  asset="cargo-hawk-$host.tar.gz"
  printf 'Installing Hawk %s for %s into %s\n' "$HAWK_VERSION" "$host" "$hawk_bin"
  "${cargo_priority[@]}" curl --proto '=https' -fsSLo "$asset" \
    "https://github.com/astral-sh/hawk/releases/download/$HAWK_VERSION/$asset" || return
  if command -v sha256sum >/dev/null 2>&1; then
    printf '%s  %s\n' "$digest" "$asset" | sha256sum -c || return
  else
    printf '%s  %s\n' "$digest" "$asset" | shasum -a 256 -c || return
  fi
  "${cargo_priority[@]}" tar -xzf "$asset" --strip-components=1 || return
  mkdir -p "$hawk_bin" || return
  mv -f cargo-hawk cargo-hawk-driver cargo-hawk-rustdoc "$hawk_bin/"
)

run_stage() {
  local stage=$1
  shift
  "$@" || {
    local status=$?
    printf 'Failed %s (%s)\n' "$dir" "$stage" >&2
    return "$status"
  }
}

if [[ ${NT_SKIP_HAWK:-} == 1 ]]; then
  echo "Skipping Hawk (NT_SKIP_HAWK=1)"
else
  if ! bootstrap_hawk; then
    echo 'Hawk setup failed. Rerun ./check.sh online with rustup and curl available, or run NT_SKIP_HAWK=1 ./check.sh.' >&2
    exit 1
  fi
  hawk_cargo=$(command -v cargo)
  hawk_shim=$(mktemp -d "${TMPDIR:-/tmp}/nt-hawk.XXXXXXXX")
  trap 'rm -rf -- "$hawk_shim"' EXIT
  # Hawk invokes `cargo test --doc` even for binary-only workspaces. Treat Cargo's
  # missing-library result as an empty doctest pass. Real doctests and other failures still run.
  cat > "$hawk_shim/cargo" <<'EOF'
#!/usr/bin/env bash
if [[ ${1:-} == test ]]; then
  for arg in "$@"; do
    if [[ $arg == --doc ]]; then
      status=0
      { stderr=$("$NT_HAWK_CARGO" "$@" 2>&1 1>&3) || status=$?; } 3>&1
      if [[ $status == 101 && $stderr != *$'\n'* &&
            $stderr == 'error: no library targets found in package `'*'`' ]]; then
        exit 0
      fi
      if [[ -n $stderr ]]; then printf '%s\n' "$stderr" >&2; fi
      exit "$status"
    fi
  done
fi
exec "$NT_HAWK_CARGO" "$@"
EOF
  chmod +x "$hawk_shim/cargo"
fi

for dir in core tools/nt-input tools/nt-explore apps/iced apps/gpui apps/rune; do
  echo "== $dir"
  (
    cd "$dir"
    if [[ "${1:-}" == "--fix" ]]; then run_stage rustfmt cargo fmt; fi
    run_stage rustfmt cargo fmt --check
    run_stage Clippy "${cargo_priority[@]}" cargo clippy --workspace --all-targets --all-features -- -D warnings
    if [[ ${NT_SKIP_HAWK:-} != 1 ]]; then
      run_stage Hawk env PATH="$hawk_bin:$hawk_shim:$PATH" NT_HAWK_CARGO="$hawk_cargo" \
        "${cargo_priority[@]}" cargo "+$HAWK_TOOLCHAIN" hawk check \
          --target-dir "$hawk_cache/target/${dir##*/}" -D warnings
    fi
  )
done
