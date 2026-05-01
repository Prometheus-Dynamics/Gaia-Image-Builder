#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
image_path="$repo_root/.gaia/examples/imported-raw-image-mutate/out/images/imported-raw-image-mutate-2.0.0.img"
mount_dir="$(mktemp -d)"
loop_device=""

cleanup() {
  set +e
  if mountpoint -q "$mount_dir"; then
    umount "$mount_dir"
  fi
  if [[ -n "$loop_device" ]]; then
    losetup -d "$loop_device"
  fi
  rmdir "$mount_dir" 2>/dev/null || true
}
trap cleanup EXIT

test -f "$image_path"
loop_device="$(losetup --find --show "$image_path")"
mount "$loop_device" "$mount_dir"

grep -qx "Gaia starting-point raw image smoke" "$mount_dir/etc/motd"
test -x "$mount_dir/usr/bin/raw-smoke-app"
test -x "$mount_dir/usr/bin/curl"

file "$image_path"
echo "verified raw image smoke output: $image_path"
