#!/usr/bin/env bash
# The Rust gate, same rules as ytsearch: default rustfmt, and Clippy with every warning an error.
#   ./check.sh          check everything
#   ./check.sh --fix    apply rustfmt first
set -euo pipefail
cd "$(dirname "$0")"
fail=0
for dir in core tools/nt-input apps/iced apps/gpui apps/rune; do
  echo "== $dir"
  if [[ "${1:-}" == "--fix" ]]; then (cd "$dir" && cargo fmt); fi
  (cd "$dir" && cargo fmt --check) || fail=1
  (cd "$dir" && cargo clippy --workspace --all-targets --all-features -- -D warnings) || fail=1
done
exit $fail
