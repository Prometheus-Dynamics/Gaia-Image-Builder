#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
archive_path="$repo_root/.gaia/examples/imported-rootfs-polyglot-git/out/images/imported-rootfs-polyglot-git-2.0.0.tar"
extract_dir="$(mktemp -d)"

cleanup() {
  rm -rf "$extract_dir"
}
trap cleanup EXIT

test -f "$archive_path"
tar -xf "$archive_path" -C "$extract_dir"
root="$extract_dir/rootfs"

grep -qx "Gaia starting-point polyglot git smoke" "$root/etc/motd"
grep -qx "APP_MODE=polyglot-git-smoke" "$root/etc/default/runtime.env"
grep -qx "FEATURE_SET=rust,go,java,node,python" "$root/etc/default/runtime.env"
test -f "$root/etc/systemd/system/polyglot.service"
test -x "$root/usr/bin/rust-app"
test -x "$root/usr/bin/go-app"
test -f "$root/usr/lib/example/java-app.jar"
test -f "$root/opt/example/node-app.tgz"
test -f "$root/opt/example/python-app.whl"

file "$archive_path"
echo "verified polyglot git smoke output: $archive_path"
