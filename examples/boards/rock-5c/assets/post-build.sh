#!/bin/sh
# Buildroot post-build: the extlinux.conf U-Boot's distro boot reads from
# the bootable rootfs-a partition, and rkbin's licence: the image carries
# its DDR init and BL31, and the licence asks for its notice with them.
set -e
install -m 0644 -D "$(dirname "$0")/extlinux.conf" "$TARGET_DIR/boot/extlinux/extlinux.conf"
rkbin="${3:-$(ls -d "$BUILD_DIR"/rockchip-rkbin-* | head -n 1)}"
install -m 0644 -D "$rkbin/LICENSE" "$TARGET_DIR/usr/share/licenses/rkbin/LICENSE"
# Optional: assets/authorized_keys (not in the repo) for root's SSH login.
if [ -f "$(dirname "$0")/authorized_keys" ]; then
	install -m 0600 -D "$(dirname "$0")/authorized_keys" "$TARGET_DIR/root/.ssh/authorized_keys"
fi
