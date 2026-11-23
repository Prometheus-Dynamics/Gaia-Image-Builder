#!/usr/bin/env bash
# The check every PR into next must pass. CI runs exactly this.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"

# One target directory, one worktree: refused before cargo runs (see the
# script for why).
scripts/gate-target.sh

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

# The tests: every test binary's cases at once under cargo-nextest (each case
# its own process), then the doc tests, which nextest does not run. Without
# nextest, cargo test runs the same cases, one binary after another.
tests() {
  if cargo nextest --version > /dev/null 2>&1; then
    # Both run and both statuses count: the step fails if either fails,
    # whether or not errexit applies where the step is called.
    local cases=0 docs=0
    cargo nextest run --locked --workspace --all-features || cases=$?
    cargo test --locked --workspace --all-features --doc || docs=$?
    if [ "$cases" -ne 0 ] || [ "$docs" -ne 0 ]; then
      echo "tests failed: nextest exit $cases, doc tests exit $docs" >&2
      return 1
    fi
  else
    echo "cargo-nextest not installed: cargo test runs the binaries one at a time"
    cargo test --locked --workspace --all-features
  fi
}

if compgen -G "crates/*/*/Cargo.toml" > /dev/null; then
  step lock lock
  step format cargo fmt --all --check
  step clippy cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
  step tests tests
else
  echo "no crates yet"
fi

# Gaia's code names no board, product or camera. Those live in descriptions.
# The names are in scripts/product-names; scripts/check.sh reads the same file.
if compgen -G "crates/*/*/src" > /dev/null; then
  step "no product names in code" bash -c '! grep -rn -i -E -f scripts/product-names crates/*/*/src'
fi

if [ -x scripts/check-boundaries.sh ]; then
  step boundaries scripts/check-boundaries.sh
fi

# `gaia store verify` on a small store this step makes in its own temp
# directory: two file objects, each named by its sha256 as the store lays
# them out. It must exit 0. Then one object is rewritten, and it must exit 2,
# so the check is not passing by accident. The gate never walks a real store.
store_verify() {
  cargo build --locked -q -p gaia
  local gaia="${CARGO_TARGET_DIR:-target}/debug/gaia" dir first h rc
  dir=$(mktemp -d "${TMPDIR:-/tmp}/gate-store-verify.XXXXXX")
  for text in alpha beta; do
    printf '%s\n' "$text" > "$dir/tmp"
    h=$(sha256sum "$dir/tmp" | cut -d' ' -f1)
    mkdir -p "$dir/store/objects/file/${h:0:2}"
    mv "$dir/tmp" "$dir/store/objects/file/${h:0:2}/$h"
    first=${first:-$dir/store/objects/file/${h:0:2}/$h}
  done
  # No RETURN trap: it would fire again when the caller returns, with no
  # `dir`. The directory is removed on each way out.
  rc=0
  "$gaia" store verify --store "$dir/store" || rc=$?
  if [ "$rc" != 0 ]; then
    rm -rf -- "$dir"
    echo "store verify: a store whose objects match must exit 0, exited $rc" >&2
    return 1
  fi
  printf 'gamma\n' > "$first"
  rc=0
  "$gaia" store verify --store "$dir/store" > /dev/null || rc=$?
  rm -rf -- "$dir"
  if [ "$rc" != 2 ]; then
    echo "store verify: an object that does not match must exit 2, exited $rc" >&2
    return 1
  fi
}

if compgen -G "crates/*/*/Cargo.toml" > /dev/null; then
  step "store verify" store_verify
fi

echo "gate passed"
