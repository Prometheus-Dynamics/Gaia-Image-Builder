#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
example_dir="$repo_root/examples/imported-rootfs-rust-git"
seed_dir="$example_dir/seed"
seed_rootfs="$seed_dir/rootfs"
seed_tar="$seed_dir/base-rootfs.tar"
seed_repo="$example_dir/seed-app-repo"
repo_template="$example_dir/repo-template"

rm -rf "$seed_dir" "$seed_repo" "$example_dir/build" "$example_dir/out"
mkdir -p "$seed_dir"

cp -R "$example_dir/seed-rootfs-template" "$seed_rootfs"
tar -cf "$seed_tar" -C "$seed_dir" rootfs

mkdir -p "$seed_repo"
cp -R "$repo_template/." "$seed_repo/"
git -C "$seed_repo" init -b main >/dev/null
git -C "$seed_repo" config user.name "Gaia Smoke"
git -C "$seed_repo" config user.email "gaia-smoke@example.invalid"
git -C "$seed_repo" add .
git -C "$seed_repo" commit -m "seed repo" >/dev/null
