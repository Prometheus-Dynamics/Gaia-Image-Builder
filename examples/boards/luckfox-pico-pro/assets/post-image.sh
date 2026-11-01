#!/bin/sh
# Buildroot post-image for the Luckfox Pico Pro: the vendor images the
# board's boot ROM and U-Boot read, next to Buildroot's.
#   boot.img     FIT with zImage, dtb and resource.img (the kernel tree's
#                own <dts>.img target, as the SDK builds it)
#   uboot.img, idblock.img and the maskrom loader rv1106_download_v*.bin
#                (the SDK's make.sh, in a copy that keeps rkbin beside it)
# Arguments after Buildroot's images dir: the SDK checkout, the DTS name.
set -e
sdk="${2:?usage: post-image.sh <images dir> <luckfox sdk> <dts>}"
dts="${3:?usage: post-image.sh <images dir> <luckfox sdk> <dts>}"
cross="$sdk/tools/linux/toolchain/arm-rockchip830-linux-uclibcgnueabihf/bin/arm-rockchip830-linux-uclibcgnueabihf-"
jobs=$(nproc)

kernel=$(ls -d "$BUILD_DIR"/linux-custom 2>/dev/null || ls -d "$BUILD_DIR"/linux-* | head -n 1)
# Host programs (extract-cert) use Buildroot's host OpenSSL, as in its own
# kernel build.
make -C "$kernel" ARCH=arm CROSS_COMPILE="$cross" MAKE_MODULES=n \
	HOSTCC="gcc -I$HOST_DIR/include -L$HOST_DIR/lib -Wl,-rpath,$HOST_DIR/lib" \
	BOOT_ITS="$kernel/boot.its" -j"$jobs" "$dts.img"
install -m 0644 "$kernel/boot.img" "$BINARIES_DIR/boot.img"

uboot="$BUILD_DIR/vendor-uboot"
mkdir -p "$uboot"
rsync -a --delete "$sdk/sysdrv/source/uboot/" "$uboot/"
cd "$uboot/u-boot"
make CROSS_COMPILE="$cross" luckfox_rv1106_uboot_defconfig rk-sfc.config
./make.sh --spl-new CROSS_COMPILE="$cross"
install -m 0644 uboot.img "$BINARIES_DIR/uboot.img"
install -m 0644 rv1106_idblock_v*.img "$BINARIES_DIR/idblock.img"
for loader in rv1106_download_v*.bin; do
	install -m 0644 "$loader" "$BINARIES_DIR/$loader"
done
