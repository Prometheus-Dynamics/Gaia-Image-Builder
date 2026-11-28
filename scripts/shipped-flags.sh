#!/usr/bin/env bash
# shipped-flags.sh: prints the CARGO_ENCODED_RUSTFLAGS a shipped (release)
# build of gaia runs with: the caller's own flags (CARGO_ENCODED_RUSTFLAGS,
# else RUSTFLAGS), then --remap-path-prefix from the cargo home to /cargo.
#
# A build records the paths of its dependencies' sources, in the cargo home
# (panic locations survive strip): unmapped, a binary carried the builder's
# home, /home/<user>/.cargo/registry/src/... and .../git/checkouts/...,
# and two homes gave two binaries. The workspace's own crates are recorded
# relative to it already. scripts/shipped-paths-check.sh builds with two
# homes and compares. (cargo's trim-paths does this, but is not stable.)
#
# Encoded flags are split on 0x1f, not spaces, so a home with a space in it
# stays one flag.
set -euo pipefail
home=${CARGO_HOME:-$HOME/.cargo}
remap="--remap-path-prefix=$home=/cargo"
if [[ -n ${CARGO_ENCODED_RUSTFLAGS:-} ]]; then
  printf '%s\037%s' "$CARGO_ENCODED_RUSTFLAGS" "$remap"
elif [[ -n ${RUSTFLAGS:-} ]]; then
  read -r -a words <<< "$RUSTFLAGS"
  (IFS=$'\037' && printf '%s\037%s' "${words[*]}" "$remap")
else
  printf '%s' "$remap"
fi
