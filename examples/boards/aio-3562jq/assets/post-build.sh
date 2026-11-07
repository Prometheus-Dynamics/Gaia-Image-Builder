#!/bin/sh
# Buildroot post-build: the extlinux.conf the vendor U-Boot's distro boot
# reads from the bootable rootfs-a partition, rkbin's licence (the image
# carries its DDR init, SPL, BL31 and OP-TEE), and an SSH key when the build
# has one. Arguments after the target dir: the rkbin tree.
set -e
here=$(dirname "$0")
rkbin="${2:?usage: post-build.sh <target> <rkbin> <firefly u-boot>}"
install -m 0644 -D "$here/extlinux.conf" "$TARGET_DIR/boot/extlinux/extlinux.conf"
install -m 0644 -D "$rkbin/LICENSE" "$TARGET_DIR/usr/share/licenses/rkbin/LICENSE"
# Optional: assets/authorized_keys (not in the repo) for root's SSH login.
if [ -f "$here/authorized_keys" ]; then
	install -m 0600 -D "$here/authorized_keys" "$TARGET_DIR/root/.ssh/authorized_keys"
fi
