#!/bin/sh
# Buildroot post-build for the Luckfox Pico Pro: the licence of the SDK's
# rkbin, whose DDR init and SPL are in the board's idblock and loader.
# Arguments after Buildroot's target dir: the SDK checkout, the DTS name.
set -e
sdk="${2:?usage: post-build.sh <target dir> <luckfox sdk> <dts>}"
install -m 0644 -D "$sdk/sysdrv/source/uboot/rkbin/LICENSE" "$TARGET_DIR/usr/share/licenses/rkbin/LICENSE"
