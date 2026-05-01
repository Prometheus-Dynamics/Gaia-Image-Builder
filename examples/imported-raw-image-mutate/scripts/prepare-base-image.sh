#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
example_dir="$repo_root/examples/imported-raw-image-mutate"
seed_dir="$example_dir/seed"
rootfs_tar="$seed_dir/alpine-minirootfs.tar.gz"
raw_image="$seed_dir/base.img"
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

mkdir -p "$seed_dir"
rm -f "$raw_image"
rm -rf "$example_dir/build" "$example_dir/out"

curl -fsSL \
  "https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/x86_64/alpine-minirootfs-3.20.5-x86_64.tar.gz" \
  -o "$rootfs_tar"

dd if=/dev/zero of="$raw_image" bs=1M count=256 status=none
loop_device="$(losetup --find --show "$raw_image")"
mkfs.ext4 -F "$loop_device" >/dev/null
mount "$loop_device" "$mount_dir"
tar -xzf "$rootfs_tar" -C "$mount_dir"
echo "raw-smoke" > "$mount_dir/etc/hostname"
sed -i 's#https://#http://#g' "$mount_dir/etc/apk/repositories"
