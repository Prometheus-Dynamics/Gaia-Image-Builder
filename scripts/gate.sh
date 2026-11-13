#!/usr/bin/env bash
# The check every PR into next must pass. CI runs exactly this.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"

step() {
  local label="$1" start
  shift
  echo "==> $label"
  start=$SECONDS
  "$@"
  echo "    $label: $((SECONDS - start)) s"
}

# The workspace glob matches nothing until the first crate lands.
# The committed Cargo.lock is what is built and tested. A manifest change
# without its lock change stops here.
lock() {
  if ! cargo metadata --locked --format-version 1 > /dev/null; then
    echo "Cargo.lock does not match the manifests." >&2
    echo "Run: cargo metadata --format-version 1 > /dev/null   (it updates only what the manifests changed)" >&2
    echo "Then commit Cargo.lock with your change." >&2
    return 1
  fi
}

if compgen -G "crates/*/*/Cargo.toml" > /dev/null; then
  step lock lock
  step format cargo fmt --all --check
  step clippy cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
  step tests cargo test --locked --workspace --all-features
else
  echo "no crates yet"
fi

# Gaia's code names no board, product or camera. Those live in descriptions.
if compgen -G "crates/*/*/src" > /dev/null; then
  step "no product names in code" bash -c '! grep -rn -i -E "\b(raze|helios|photonvision|ov9782|ov9281|systemcore|roborio)\b" crates/*/*/src'
fi

if [ -x scripts/check-boundaries.sh ]; then
  step boundaries scripts/check-boundaries.sh
fi

echo "gate passed"
