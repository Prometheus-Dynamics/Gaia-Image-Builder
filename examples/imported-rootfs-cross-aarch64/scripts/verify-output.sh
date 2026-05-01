#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
archive_path="$repo_root/.gaia/examples/imported-rootfs-cross-aarch64/out/images/imported-rootfs-cross-aarch64-2.0.0.tar"
extract_dir="$(mktemp -d)"

cleanup() {
  rm -rf "$extract_dir"
}
trap cleanup EXIT

test -f "$archive_path"
tar -xf "$archive_path" -C "$extract_dir"
root="$extract_dir/rootfs"

grep -qx "Gaia starting-point cross-target git smoke" "$root/etc/motd"
test -x "$root/usr/bin/rust-app"
test -x "$root/usr/bin/go-app"

rust_file="$(file "$root/usr/bin/rust-app")"
go_file="$(file "$root/usr/bin/go-app")"

printf '%s\n' "$rust_file" | grep -q "ARM aarch64"
printf '%s\n' "$go_file" | grep -q "ARM aarch64"

file "$archive_path"
echo "verified cross-target git smoke output: $archive_path"
