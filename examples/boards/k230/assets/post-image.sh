#!/bin/sh
# Buildroot post-image for the K230D boards (the K230 family): the SPL and
# U-Boot headers the Canaan boot ROM reads, made from the build's own U-Boot
# tree and the SDK's host tools. Gaia's partition table reads these files by
# name from the OS's bootloader output, which sorts a file into that bucket
# only when its name starts with u-boot, spl, idbloader, MLO, bl31 or tf-a;
# so the names here do: spl-fn.bin and u-boot-fn.bin, and u-boot-env.env.
#
# Arguments: the images dir (from Buildroot), the board's dtb name (unused:
# the kernel step makes the dtb), the SDK root (the K230 Linux SDK checkout,
# whose tools/k230_priv_gzip is checked against its pinned sha256).
#
# The SDK's U-Boot board files are not in this tree: the family's [bootloader]
# patch (k230/assets/patches) carries them. The burner loader_mmc.bin (the
# USB burner, for the boot ROM over USB) is still made here, as the SDK does.
set -e
sdk="${3:?usage: post-image.sh <images dir> <dtb name> <sdk root>}"
priv_gzip="$sdk/tools/k230_priv_gzip"
want=c6d029a05f2d3038fd02f9b18716b595f4e8beeebca33f3598328fbde99bf11e
got=$(sha256sum "$priv_gzip" | cut -d' ' -f1)
[ "$got" = "$want" ] || { echo "post-image: $priv_gzip sha256 is $got, want $want" >&2; exit 1; }
uboot_build=$(ls -d "$BUILD_DIR"/uboot-* | head -n 1)
mkimage="$uboot_build/tools/mkimage"
firmware_gen="python3 $uboot_build/tools/firmware_gen_no_securiy.py"
base=$(awk '/CONFIG_MEM_LINUX_SYS_BASE/ { print $3 }' \
	"$uboot_build/board/canaan/common/sdk_autoconf.h")

# The SDK's gzip writes the stream the boot ROM and U-Boot accept; it tries
# levels until one fits, as the SDK's script does.
k230_gzip() {
	for level in 8 9 7 6 5 4; do
		"$priv_gzip" -n"$level" -f -k "$1" && break
	done
	sed -i -e '1s/\x08/\x09/' "$1.gz"
}

mkdir -p "$BINARIES_DIR/uboot-work"
cd "$BINARIES_DIR/uboot-work"
cp "$uboot_build/u-boot.bin" "$uboot_build/spl/u-boot-spl.bin" .

k230_gzip u-boot.bin
"$mkimage" -A riscv -C gzip -O u-boot -T firmware -a "$base" -e "$base" \
	-n uboot -d u-boot.bin.gz ug_u-boot.bin
$firmware_gen -i ug_u-boot.bin -o "$BINARIES_DIR/u-boot-fn.bin" -n
$firmware_gen -i u-boot-spl.bin -o "$BINARIES_DIR/spl-fn.bin" -n
"$uboot_build/tools/mkenvimage" -s 0x10000 -o "$BINARIES_DIR/u-boot-env.env" \
	"$sdk/buildroot-overlay/board/canaan/k230-soc/default.env"

# The USB burner: the SDK's burntool U-Boot, built from a copy of the board
# U-Boot tree so that build stays as Buildroot left it (links at 0x80300000).
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
cd "$BINARIES_DIR" && rm -rf uboot-work
