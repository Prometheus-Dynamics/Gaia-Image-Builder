#!/bin/sh
# Buildroot post-image for the K230D Zero: the boot files the SDK's
# board/canaan/k230-soc/post-image.sh makes, without its own SD image
# (Gaia assembles that) and without editing the SDK tree. The SDK script
# finds the SDK two levels above BASE_DIR, which only holds in the SDK's
# own output/<defconfig> layout.
#
# Arguments: the images dir (from Buildroot), the dtb name, the SDK root.
# Writes images/uboot/fn_u-boot-spl.bin (SPL with the K230 firmware head),
# images/uboot/fn_ug_u-boot.bin (gzip U-Boot with an image header and the
# firmware head), images/uboot/env.env and images/boot.ext4 (OpenSBI with
# U-Boot's header, the kernel and the dtb, which the env's blinux loads
# from partition 1) and images/loader_mmc.bin, the USB burner.
set -e
dtb="${2:?usage: post-image.sh <images dir> <dtb name> <sdk root>}.dtb"
sdk="${3:?usage: post-image.sh <images dir> <dtb name> <sdk root>}"
uboot_build=$(ls -d "$BUILD_DIR"/uboot-* | head -n 1)
board="$sdk/output/buildroot-2024.02.1/board/canaan/k230-soc"
mkimage="$uboot_build/tools/mkimage"
firmware_gen="python3 $uboot_build/tools/firmware_gen_no_securiy.py"
base=$(awk '/CONFIG_MEM_LINUX_SYS_BASE/ { print $3 }' \
	"$uboot_build/board/canaan/common/sdk_autoconf.h")

# The SDK's gzip writes the stream its boot ROM and U-Boot accept; it
# tries levels until one fits, as the SDK script does.
k230_gzip() {
	for level in 8 9 7 6 5 4; do
		"$sdk/tools/k230_priv_gzip" -n"$level" -f -k "$1" && break
	done
	sed -i -e '1s/\x08/\x09/' "$1.gz"
}

mkdir -p "$BINARIES_DIR/uboot"
cd "$BINARIES_DIR/uboot"
cp "$uboot_build/u-boot.bin" "$uboot_build/spl/u-boot-spl.bin" .

k230_gzip u-boot.bin
"$mkimage" -A riscv -C gzip -O u-boot -T firmware -a "$base" -e "$base" \
	-n uboot -d u-boot.bin.gz ug_u-boot.bin
$firmware_gen -i ug_u-boot.bin -o fn_ug_u-boot.bin -n
$firmware_gen -i u-boot-spl.bin -o fn_u-boot-spl.bin -n
rm -f u-boot.bin.gz ug_u-boot.bin

"$uboot_build/tools/mkenvimage" -s 0x10000 -o env.env "$board/default.env"

# The burner the K230 boot ROM runs over USB to write the TF card (or
# SPI-NAND): the SDK's U-Boot with its burntool defconfig, whose plain
# u-boot.bin is the loader, per Canaan's burning guide. Built from a copy
# of the board U-Boot tree so that build stays as Buildroot left it. It
# links at 0x80300000 (CONFIG_SYS_TEXT_BASE), not k230-flash's default
# 0x80360000.
burner="$BUILD_DIR/uboot-burntool"
rm -rf "$burner"
cp -a "$uboot_build" "$burner"
burner_make() {
	make -C "$burner" ARCH=riscv \
		CROSS_COMPILE="$HOST_DIR/bin/riscv64-unknown-linux-gnu-" \
		HOSTCC="gcc -std=gnu17 -isystem $HOST_DIR/include" \
		HOSTLDFLAGS="-L$HOST_DIR/lib -Wl,-rpath,$HOST_DIR/lib" "$@"
}
burner_make k230d_canmv_burntool_defconfig
burner_make -j"$(nproc)" u-boot.bin
cp "$burner/u-boot.bin" "$BINARIES_DIR/loader_mmc.bin"

cd "$BINARIES_DIR"
rm -rf boot boot.ext4
mkdir boot
cp Image "$dtb" boot/
ln -s "$dtb" boot/k.dtb
"$mkimage" -A riscv -O linux -T kernel -C none -a 0 -e 0 -n linux \
	-d fw_jump.bin boot/fw_jump_add_uboot_head.bin
# Under fakeroot, so the files are root's rather than the builder's.
"$HOST_DIR/bin/fakeroot" -- "$HOST_DIR/sbin/mkfs.ext4" -d boot -r 1 -N 0 -m 1 \
	-L boot -O ^64bit boot.ext4 45M
