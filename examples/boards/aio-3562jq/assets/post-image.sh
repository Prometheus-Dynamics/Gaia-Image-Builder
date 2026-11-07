#!/bin/sh
# Buildroot post-image for the AIO-3562JQ: Firefly's vendor U-Boot, built
# with its make.sh in a copy that has rkbin beside it (as the SDK lays them
# out), with Buildroot's aarch64 toolchain:
#   uboot.img     FIT with U-Boot, BL31 and OP-TEE (rkbin RK3562TRUST.ini),
#                 read by the SPL from 8 MiB
#   idblock.bin   rkbin's DDR init and SPL (RK3562MINIALL.ini), at 32 KiB
#   rk3562_loader_v*.bin   the maskrom loader, for rkdeveloptool and Atlas
# Arguments after Buildroot's images dir: the rkbin tree, the U-Boot tree.
set -e
rkbin="${2:?usage: post-image.sh <images dir> <rkbin> <firefly u-boot>}"
src="${3:?usage: post-image.sh <images dir> <rkbin> <firefly u-boot>}"
work="$BUILD_DIR/vendor-uboot"
mkdir -p "$work"
rsync -a --delete "$src/" "$work/u-boot/"
ln -sfn "$rkbin" "$work/rkbin"
cd "$work/u-boot"
# make.sh: Buildroot's job count, python3 (decode_bl31.py is python3-clean;
# the SDK assumes python2), and GCC 14 warnings that this 2017 tree turns
# into errors (-Wno-error through KCFLAGS).
sed -i "s/^JOB=.*/JOB=${BR2_JLEVEL:-$(nproc)}/; s/which python2/which python3/; s/PYTHON=python2/PYTHON=python3/" make.sh
sed -i 's|^\t${srctree}/arch/arm/mach-rockchip/decode_bl31.py|\tpython3 ${srctree}/arch/arm/mach-rockchip/decode_bl31.py|' arch/arm/mach-rockchip/fit_nodes.sh
PATH="$HOST_DIR/bin:$PATH" KCFLAGS="-Wno-error" ./make.sh rk3562 CROSS_COMPILE=aarch64-buildroot-linux-gnu-
ini="$rkbin/RKBOOT/RK3562MINIALL.ini"
ddr="$rkbin/$(sed -n 's/^FlashData=//p' "$ini" | tr -d '\r')"
spl="$rkbin/$(sed -n 's/^FlashBoot=//p' "$ini" | tr -d '\r')"
./tools/mkimage -n rk3562 -T rksd -d "$ddr:$spl" idblock.bin
install -m 0644 uboot.img "$BINARIES_DIR/uboot.img"
install -m 0644 idblock.bin "$BINARIES_DIR/idblock.bin"
for loader in rk3562_loader_v*.bin; do
	install -m 0644 "$loader" "$BINARIES_DIR/$loader"
done
install -m 0644 "$rkbin/LICENSE" "$BINARIES_DIR/rkbin-LICENSE.txt"
