#!/usr/bin/env bash
set -euo pipefail

export RUSTFLAGS="${RUSTFLAGS:-} -Dwarnings"

cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features

if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny check
else
  echo "skip cargo-deny: cargo-deny not installed" >&2
fi

if command -v npx >/dev/null 2>&1; then
  npx -y slop-scan scan . --lint
else
  echo "skip slop-scan: npx not installed" >&2
fi
