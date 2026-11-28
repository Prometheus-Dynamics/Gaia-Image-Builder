#!/usr/bin/env bash
# gate-home.sh HOME: fills HOME, a CARGO_HOME of the gate's own, with the
# crates of every committed Cargo.lock, so the gate's lanes run offline in
# it and never wait on another program's lock on ~/.cargo's package cache.
# scripts/gate.sh runs it first when GAIA_GATE_CARGO_HOME names a home.
#
# HOME's config.toml sets cargo offline and turns its automatic cleaning of
# the package cache off, so no lane downloads or takes the cache's lock to
# clean it. Here, and only here, cargo goes online: `cargo fetch --locked`
# for each lock file. HOME/fetch.log records each lock file's sha256, the
# sources (the registry, each git source at its commit), the number of crate
# files and a sha256 over their sums. HOME/fetch.key is the sha256 over the
# lock files' sums: when it matches, nothing is fetched and the network is
# not touched. A failed fetch exits 2, before any lane.
#
# Keep HOME at one path: builds record the paths of their crates' sources
# (panic locations, debug info), so a new path rebuilds every dependency and
# moves the code the cases are keyed on.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
home=${1:?usage: gate-home.sh HOME}
[[ $home == /* ]] || { echo "gate-home.sh: HOME must be an absolute path, not $home" >&2; exit 2; }
mkdir -p "$home"
printf '%s\n' "# Written by scripts/gate-home.sh: the gate's own CARGO_HOME." \
  "[net]" "offline = true" "[cache]" 'auto-clean-frequency = "never"' > "$home/config.toml.new"
if cmp -s "$home/config.toml.new" "$home/config.toml"; then
  rm -f "$home/config.toml.new"
else
  mv "$home/config.toml.new" "$home/config.toml"
fi

locks=$(git ls-files '*Cargo.lock' | LC_ALL=C sort)
key=$(for lock in $locks; do sha256sum "$lock"; done | sha256sum | cut -c1-64)
if [[ -f $home/fetch.key && $(cat "$home/fetch.key") == "$key" ]]; then
  echo "gate home: $home holds the crates of the lock files (fetch key ${key:0:16}): nothing to fetch"
  exit 0
fi

fetch() {
  local lock rc=0
  echo "fetch: CARGO_HOME=$home"
  for lock in $locks; do
    echo "fetch: $lock sha256 $(sha256sum "$lock" | cut -d' ' -f1)"
    CARGO_HOME=$home cargo fetch --locked --config net.offline=false \
      --manifest-path "$(dirname "$lock")/Cargo.toml" || { echo "fetch: $lock: cargo fetch failed"; rc=1; }
  done
  # shellcheck disable=SC2086 # the lock file paths have no spaces
  echo "fetch: sources: $(sed -n 's/^source = "\([a-z]*+[^?#"]*\)[^#"]*\(#[0-9a-f]*\)\{0,1\}".*/\1\2/p' $locks | LC_ALL=C sort -u | tr '\n' ' ')"
  local crates
  crates=$(cd "$home/registry/cache" 2> /dev/null && find . -name '*.crate' -print0 | LC_ALL=C sort -z | xargs -0r sha256sum)
  echo "fetch: $(printf '%s' "$crates" | grep -c .) registry crate files, sha256 over their sums $(printf '%s\n' "$crates" | sha256sum | cut -c1-64)"
  echo "fetch: $(find "$home/git/checkouts" -mindepth 2 -maxdepth 2 -type d 2> /dev/null | wc -l) git checkouts"
  return "$rc"
}
if ! fetch > "$home/fetch.log" 2>&1; then
  echo "gate home: the crates could not be fetched into $home (its fetch.log):" >&2
  tail -5 "$home/fetch.log" >&2
  exit 2
fi
echo "$key" > "$home/fetch.key"
grep -E '^fetch: ([0-9]+ registry|[0-9]+ git|sources)' "$home/fetch.log"
