#!/usr/bin/env bash
# The check every PR into next must pass. CI runs exactly this.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
# One niceness for all heavy work on the host (see the script).
source scripts/nice.sh
# Incremental compilation on a kept target (here: an edit to gaia-store, the
# workspace's test binaries rebuilt, interleaved 3 and 3 at load 120-149:
# 127 CPU-s and 20.1 s against 219 CPU-s and 27.6 s). Off on CI, which
# builds from nothing, where it only writes more.
if [ -n "${GITHUB_ACTIONS:-}" ]; then
  export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
else
  export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-1}"
fi

# The commit gate-built binaries carry in `gaia --version`: plainly not a
# commit. The real HEAD (with -dirty) differs with every commit and edit,
# and as a string in the binary it moves the loaded code of every case that
# runs gaia, so those cases ran again whatever changed (gaia-verdict keys on
# that code). Builds outside the gate embed the real HEAD (build.rs).
export GAIA_COMMIT=gate

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

# What the tests have to cover (scripts/gate-lanes.py): nothing when this
# code tree passed already, the packages a change can reach when a passed
# ancestor is known, else everything. GAIA_GATE_FULL=1 runs everything.
plan="full asked for by GAIA_GATE_FULL=1"
if [ "${GAIA_GATE_FULL:-}" != 1 ]; then
  plan=$(scripts/gate-lanes.py plan)
fi
echo "==> plan: $plan"
case "$plan" in
  none\ *)
    echo "gate passed (this code tree passed at ${plan#none }; nothing it tests changed)"
    exit 0
    ;;
  lanes\ *)
    read -r -a lane_words <<< "${plan#lanes }"
    packages=() doc_packages=()
    for word in "${lane_words[@]:1}"; do
      packages+=(-p "${word%:bin}")
      [[ $word == *:bin ]] || doc_packages+=(-p "$word")
    done
    ;;
  *) packages=(--workspace) doc_packages=(--workspace) ;;
esac

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
# nextest the gate fails, saying so, unless GAIA_GATE_UNBOUNDED=1: cargo test
# would run the same cases with no bound on any of them.
tests() {
  if cargo nextest --version > /dev/null 2>&1; then
    # Both run and both statuses count: the step fails if either fails,
    # whether or not errexit applies where the step is called.
    local cases=0 docs=0
    # Each case runs through scripts/test-bound.py (cargo's runner for the
    # host's own target): a case that hangs is stopped at its bound and
    # fails by name, and the bound counts the time the case could use, not
    # the time it waited for a CPU on a busy machine (see the script).
    # Only here: under plain cargo test a process is a whole binary, and
    # the bound is one case's.
    local host runner
    host=$(rustc -vV | sed -n 's/^host: //p')
    runner="CARGO_TARGET_$(printf '%s' "$host" | tr 'a-z-' 'A-Z_')_RUNNER"
    # On a CI runner every case runs to its end: one red run names every
    # case that fails there, not just the first.
    local all=()
    if [ -n "${GITHUB_ACTIONS:-}" ]; then all=(--no-fail-fast); fi
    env "$runner=$PWD/scripts/test-bound.py" \
      cargo nextest run --locked "${packages[@]}" "${all[@]}" --all-features || cases=$?
    if [ "${#doc_packages[@]}" -gt 0 ]; then
      cargo test --locked "${doc_packages[@]}" --all-features --doc || docs=$?
    fi
    if [ "$cases" -ne 0 ] || [ "$docs" -ne 0 ]; then
      echo "tests failed: nextest exit $cases, doc tests exit $docs" >&2
      return 1
    fi
  elif [ "${GAIA_GATE_UNBOUNDED:-}" = 1 ]; then
    # Asked for by name: without nextest a process is a whole test binary,
    # so no case has a bound and a hung one holds this run.
    echo "cargo-nextest is not installed, and GAIA_GATE_UNBOUNDED=1: the tests run under plain cargo test, WITH NO BOUND on any test" >&2
    cargo test --locked "${packages[@]}" --all-features
  else
    echo "cargo-nextest is not installed: the gate's tests run under it, each case bounded (scripts/test-bound.py)." >&2
    echo "Install it (cargo install cargo-nextest --locked), or set GAIA_GATE_UNBOUNDED=1 to run the tests with no bound on any of them." >&2
    return 1
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

scripts/gate-lanes.py passed
echo "gate passed"
