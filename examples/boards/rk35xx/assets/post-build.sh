#!/bin/sh
# Buildroot post-build for every RK35xx board. Arguments after the target
# dir (BR2_ROOTFS_POST_SCRIPT_ARGS, set by the board's layer): the RKBOOT ini
# (post-image's) and the device tree the kernel boots with, as
# `<vendor>/<name>` without `.dtb`, then the console if it is not ttyS2 at
# 1500000 baud (the A733 base shares this script).
# - extlinux.conf for U-Boot's distro boot on the bootable rootfs-a.
# - rkbin's licence, on a board that uses it: the image carries its DDR init.
# - An SSH key for root when the build has one: authorized_keys next to
#   this script (git-ignored).
# A board adds its own steps as a second script in BR2_ROOTFS_POST_BUILD_SCRIPT.
set -e
here=$(dirname "$0")
dtb="${3:?usage: post-build.sh <target dir> <RKBOOT ini> <vendor/dts name>}"
console="${4:-ttyS2,1500000}"
mkdir -p "$TARGET_DIR/boot/extlinux"
cat > "$TARGET_DIR/boot/extlinux/extlinux.conf" <<CONF
default rootfs-a
label rootfs-a
  kernel /boot/Image
  devicetree /boot/${dtb##*/}.dtb
  append root=PARTLABEL=rootfs-a rootwait console=$console earlycon
CONF
rkbin=$(ls -d "$BUILD_DIR"/rockchip-rkbin-* 2>/dev/null | head -n 1)
if [ -n "$rkbin" ]; then
	install -m 0644 -D "$rkbin/LICENSE" "$TARGET_DIR/usr/share/licenses/rkbin/LICENSE"
fi
if [ -f "$here/authorized_keys" ]; then
	install -m 0600 -D "$here/authorized_keys" "$TARGET_DIR/root/.ssh/authorized_keys"
fi
