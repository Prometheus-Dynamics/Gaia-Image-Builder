#!/usr/bin/env bash
# The inner loop: the gate's own steps for the crates a change touches and
# every workspace crate that depends on them, nothing else.
#
#   scripts/check.sh           changes since the merge base with origin/next
#   scripts/check.sh BASE      changes since BASE (a commit, branch or tag)
#
# Changed files are the commits since the merge base, the working tree and
# untracked files. If any of them is outside crates/<group>/<crate>/, or is
# a crate's Cargo.toml (features and dependencies reach other crates), this
# runs the whole gate instead.
#
# Each step is the gate's command with the workspace narrowed to those
# crates by -p, so a red here is red in the gate. Green here is not the
# gate: features unify differently when fewer crates are built, and lock,
# boundaries and store verify are not run. Run scripts/gate.sh before
# landing.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
source scripts/nice.sh
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"

base_ref="${1:-origin/next}"
base=$(git merge-base HEAD "$base_ref")
mapfile -t changed < <({
  git diff --name-only "$base"
  git ls-files --others --exclude-standard
} | sort -u)

if [ "${#changed[@]}" -eq 0 ]; then
  echo "no changes since $(git rev-parse --short "$base")"
  exit 0
fi

dirs=()
for file in "${changed[@]}"; do
  if [[ "$file" =~ ^(crates/[^/]+/[^/]+)/ ]] && [[ "$file" != */Cargo.toml ]]; then
    dirs+=("${BASH_REMATCH[1]}")
  else
    echo "$file is outside a crate's sources: running the whole gate"
    exec scripts/gate.sh
  fi
done

# The crates in those directories, then everything in the workspace that
# depends on them, directly or not (dev-dependencies count: their tests).
mapfile -t packages < <(cargo metadata --locked --no-deps --format-version 1 |
  python3 -c '
import json, os, sys
meta = json.load(sys.stdin)
root = meta["workspace_root"]
dirs = {os.path.join(root, d) for d in sys.argv[1:]}
by_name = {p["name"]: p for p in meta["packages"]}
users = {name: set() for name in by_name}
for p in by_name.values():
    for dep in p["dependencies"]:
        if dep["name"] in by_name:
            users[dep["name"]].add(p["name"])
todo = [n for n, p in by_name.items() if os.path.dirname(p["manifest_path"]) in dirs]
seen = set(todo)
while todo:
    for user in users[todo.pop()]:
        if user not in seen:
            seen.add(user)
            todo.append(user)
print("\n".join(sorted(seen)))
' "${dirs[@]}")

if [ "${#packages[@]}" -eq 0 ]; then
  echo "no workspace crate in: ${dirs[*]}: running the whole gate"
  exec scripts/gate.sh
fi

echo "crates: ${packages[*]}"
selected=()
for package in "${packages[@]}"; do
  selected+=(-p "$package")
done

step() {
  local label="$1" start
  shift
  echo "==> $label"
  start=$SECONDS
  "$@"
  echo "    $label: $((SECONDS - start)) s"
}

tests() {
  local cases=0 docs=0
  if cargo nextest --version > /dev/null 2>&1; then
    cargo nextest run --locked "${selected[@]}" --all-features --no-tests=pass || cases=$?
    cargo test --locked "${selected[@]}" --all-features --doc || docs=$?
  else
    cargo test --locked "${selected[@]}" --all-features || cases=$?
  fi
  if [ "$cases" -ne 0 ] || [ "$docs" -ne 0 ]; then
    echo "tests failed: cases exit $cases, doc tests exit $docs" >&2
    return 1
  fi
}

names() {
  local sources=()
  for dir in "${dirs[@]}"; do
    [ -d "$dir/src" ] && sources+=("$dir/src")
  done
  [ "${#sources[@]}" -eq 0 ] ||
    ! grep -rn -i -E -f scripts/product-names "${sources[@]}"
}

step format cargo fmt --check "${selected[@]}"
step clippy cargo clippy --locked "${selected[@]}" --all-targets --all-features -- -D warnings
step tests tests
step "no product names in code" names
echo "scoped check passed for ${#packages[@]} crates; run scripts/gate.sh before landing"
