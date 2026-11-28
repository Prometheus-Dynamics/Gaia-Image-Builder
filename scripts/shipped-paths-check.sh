#!/usr/bin/env bash
# shipped-paths-check.sh WORK: builds the release gaia, as shipped
# (scripts/shipped-flags.sh, --locked), from two cargo homes at two paths,
# and fails unless the two binaries are the same bytes and neither holds
# its home's path. WORK is a directory of the caller's own: it holds the
# homes (the second a copy of the first, reflinked where the disk can) and
# one target directory, emptied between the builds so that only the home
# differs. Each build is a whole release build, so this runs by hand, not
# in the gate.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
work=${1:?usage: shipped-paths-check.sh WORK}
[[ $work == /* ]] || { echo "shipped-paths-check.sh: WORK must be an absolute path, not $work" >&2; exit 2; }
a=$work/home-a b=$work/home-b-at-another-path target=$work/target
mkdir -p "$work"
scripts/gate-home.sh "$a"
rm -rf -- "$b"
cp -a --reflink=auto "$a" "$b"

build() { # HOME OUT
  rm -rf -- "$target"
  local s=$SECONDS flags
  flags=$(CARGO_HOME=$1 scripts/shipped-flags.sh)
  CARGO_HOME=$1 CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$target CARGO_ENCODED_RUSTFLAGS=$flags \
    cargo build --release --locked -p gaia --bin gaia --quiet
  cp "$target/release/gaia" "$2"
  echo "built from $1: $((SECONDS - s)) s, $(stat -c %s "$2") bytes, sha256 $(sha256sum "$2" | cut -c1-16)"
}
build "$a" "$work/gaia-a"
build "$b" "$work/gaia-b"
rm -rf -- "$target"

rc=0
for side in a b; do
  home=$a
  [[ $side == a ]] || home=$b
  n=$(grep -c -F -- "$home" "$work/gaia-$side" || true)
  echo "gaia-$side: $n lines with its home's path"
  [[ $n == 0 ]] || rc=1
done
if cmp "$work/gaia-a" "$work/gaia-b"; then
  echo "the two builds are the same bytes"
else
  echo "the two builds differ" >&2
  rc=1
fi
exit "$rc"
