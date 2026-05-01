#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
example_dir="$repo_root/examples/imported-rootfs-polyglot-git"
seed_dir="$example_dir/seed"
seed_rootfs="$seed_dir/rootfs"
seed_tar="$seed_dir/base-rootfs.tar"
templates="$example_dir/repo-templates"

rm -rf \
  "$seed_dir" \
  "$example_dir/seed-rust-repo" \
  "$example_dir/seed-go-repo" \
  "$example_dir/seed-java-repo" \
  "$example_dir/seed-node-repo" \
  "$example_dir/seed-python-repo" \
  "$example_dir/build.local.toml" \
  "$example_dir/build" \
  "$example_dir/out"

mkdir -p "$seed_dir"
cp -R "$example_dir/seed-rootfs-template" "$seed_rootfs"
tar -cf "$seed_tar" -C "$seed_dir" rootfs

for kind in rust go java node python; do
  repo_path="$example_dir/seed-${kind}-repo"
  mkdir -p "$repo_path"
  cp -R "$templates/$kind/." "$repo_path/"
  git -C "$repo_path" init -b main >/dev/null
  git -C "$repo_path" config user.name "Gaia Smoke"
  git -C "$repo_path" config user.email "gaia-smoke@example.invalid"
  git -C "$repo_path" add .
  git -C "$repo_path" commit -m "seed $kind repo" >/dev/null
done

build_template="$example_dir/build.toml"
build_local="$example_dir/build.local.toml"
python3 - <<'PY' "$build_template" "$build_local" "$example_dir"
from pathlib import Path
import sys

template = Path(sys.argv[1])
output = Path(sys.argv[2])
example_dir = Path(sys.argv[3]).resolve()

content = template.read_text()
for kind in ("rust", "go", "java", "node", "python"):
    rel = f"examples/imported-rootfs-polyglot-git/seed-{kind}-repo"
    abs_path = str((example_dir / f"seed-{kind}-repo").resolve())
    content = content.replace(f'repo = "{rel}"', f'repo = "{abs_path}"')

output.write_text(content)
PY
