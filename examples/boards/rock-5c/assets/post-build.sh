#!/bin/sh
# Buildroot post-build: the extlinux.conf U-Boot's distro boot reads from
# the bootable rootfs-a partition.
set -e
install -m 0644 -D "$(dirname "$0")/extlinux.conf" "$TARGET_DIR/boot/extlinux/extlinux.conf"
