#!/bin/sh
# Buildroot post-image for the K230D AI-CAM bench image (the K230 family's boot
# chain). It makes the files the family's partitions name, in BINARIES_DIR, where
# Gaia's bootloader output picks them up by name (u-boot*, spl*):
#   spl-fn.bin           SPL: the K230 first-stage header over the SPL
#   u-boot-fn.bin        U-Boot: priv gzip, legacy uImage (firmware, gzip), then the header
#   u-boot-opensbi.bin   OpenSBI fw_jump as a legacy uImage (kernel, no compression)
#   u-boot-env.bin       U-Boot environment copy 1 (ENV_OFFSET 0x300000)
#   u-boot-env-redund.bin  copy 2, the redundant form (ENV_OFFSET_REDUND 0x320000)
# Every header tool is Gaia's k230-firstage (crates/nodes/k230-image), passed as the
# second argument; mkenvimage is U-Boot's own, from the build tree.
# Nothing here runs a vendor binary.
#
# The bootloader's own binaries are plain: spl/u-boot-spl.bin and u-boot.bin in the
# U-Boot build directory (the family's patch removed the SDK's Python header steps).
#
# Untested: this script has not run in a Buildroot build yet.
set -e
# $1 is the images dir (Buildroot), $2 the tool (BR2_ROOTFS_POST_SCRIPT_ARGS).
tool="${2:?the k230-firstage tool path is the post-image's second argument}"
uboot_build=$(ls -d "$BUILD_DIR"/uboot-* | head -n 1)
dtb=k230d-ai-cam.dtb
cd "$BINARIES_DIR"

# SPL and U-Boot first-stage images.
"$tool" wrap "$uboot_build/spl/u-boot-spl.bin" spl-fn.bin
"$tool" priv-gzip "$uboot_build/u-boot.bin" u-boot.gz
"$tool" uimage --os u-boot --type firmware --comp gzip --load 0 --entry 0 --name uboot u-boot.gz u-boot.uimage
"$tool" wrap u-boot.uimage u-boot-fn.bin

# OpenSBI, the uImage U-Boot's bootm starts at 0x3000000. Its FW_JUMP address is 0x200000
# (FW_TEXT_START=0 plus the generic platform's jump offset), where the kernel is loaded.
"$tool" uimage --os linux --type kernel --comp none --load 0 --entry 0 --name linux fw_jump.bin u-boot-opensbi.bin

# The environment. The read address and block count name the OpenSBI slice at 0x380000.
blocks=$(( ($(stat -c %s u-boot-opensbi.bin) + 511) / 512 ))
cat > u-boot-env.txt <<ENV
bootdelay=3
bootcmd=mmc dev 1 && mmc read 0x3000000 0x1c00 $blocks && ext4load mmc 1:1 0x200000 /boot/Image && ext4load mmc 1:1 0x2200000 /boot/$dtb && bootm 0x3000000 - 0x2200000
bootargs=root=/dev/mmcblk0p1 rootwait rw rootfstype=ext4 console=ttyS0,115200 earlycon=sbi
ENV
"$uboot_build/tools/mkenvimage" -s 0x2000 -o u-boot-env.bin u-boot-env.txt
"$uboot_build/tools/mkenvimage" -s 0x2000 -r -o u-boot-env-redund.bin u-boot-env.txt

rm -f u-boot.gz u-boot.uimage u-boot-env.txt
