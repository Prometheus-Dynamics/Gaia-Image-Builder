#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
archive_path="$repo_root/.gaia/examples/imported-rootfs-rust-git/out/images/imported-rootfs-rust-git-2.0.0.tar"
extract_dir="$(mktemp -d)"

cleanup() {
  rm -rf "$extract_dir"
}
trap cleanup EXIT

test -f "$archive_path"
tar -xf "$archive_path" -C "$extract_dir"
root="$extract_dir/rootfs"

grep -qx "Gaia starting-point git project smoke" "$root/etc/motd"
grep -qx "APP_MODE=git-smoke" "$root/etc/default/runtime.env"
grep -qx "LISTEN_ADDR=0.0.0.0:8080" "$root/etc/default/runtime.env"
test -f "$root/etc/systemd/system/git-project.service"
test -x "$root/usr/bin/git-project-app"

file "$archive_path"
echo "verified git project smoke output: $archive_path"
