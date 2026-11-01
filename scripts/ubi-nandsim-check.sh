#!/usr/bin/env bash
# Attaches a Gaia UBI image with the kernel's UBI driver on a simulated NAND
# (nandsim) and checks its volumes, the test the byte-level read-back tests
# cannot do. Needs root, the nandsim and ubi modules, and mtd-utils' UBI
# tools (ubiformat, ubiattach, ubinfo, ubidetach; Fedora: mtd-utils-ubi).
#
#   sudo modprobe nandsim     # default: 128 MiB, 128 KiB blocks, 2 KiB pages
#   sudo scripts/ubi-nandsim-check.sh ubi.img rootfs_a=rootfs.squashfs
#
# Arguments: the UBI image, then name=file pairs: each named volume must
# start with that file's bytes. MTD selects the device (default: the first
# nandsim one). The image is flashed with ubiformat, as on a board.
#
# nandsim is raw NAND with 512-byte sub-pages, where the kernel expects the
# VID header at 512. SPI-NAND has none, so Gaia's image has it at the page
# size. The offset is read from the image's first EC header and given to
# ubiformat and ubiattach, as `ubi.mtd=<dev>,<offset>` gives it on a board.
# The chip is erased first: ubiformat otherwise keeps the headers an earlier
# run left, and its own scan overrides the offsets given here.
set -euo pipefail

image="${1:?usage: ubi-nandsim-check.sh <ubi.img> [volume=file ...]}"
shift
[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 2; }
for tool in flash_erase ubiformat ubiattach ubinfo ubidetach; do
	command -v "$tool" > /dev/null || { echo "missing $tool (mtd-utils UBI tools)" >&2; exit 2; }
done

mtd="${MTD:-}"
if [[ -z $mtd ]]; then
	mtd=$(awk -F: '/NAND simulator/ { sub("mtd", "", $1); print $1; exit }' /proc/mtd)
fi
[[ -n $mtd ]] || { echo "no nandsim MTD device; load nandsim (see the header)" >&2; exit 2; }
ubi=9
# The big-endian vid_hdr_offset of the EC header at the start of the image.
vid_offset=$((16#$(od -An -tx1 -j16 -N4 "$image" | tr -d ' \n')))

ubidetach -d "$ubi" 2> /dev/null || true
flash_erase -q "/dev/mtd$mtd" 0 0
ubiformat "/dev/mtd$mtd" -f "$image" -s "$vid_offset" -O "$vid_offset" -y -q
if ! ubiattach -m "$mtd" -d "$ubi" -O "$vid_offset" > /dev/null; then
	dmesg | grep "ubi$ubi" | tail -n 8 >&2
	exit 1
fi
trap 'ubidetach -d "$ubi" 2> /dev/null || true' EXIT
ubinfo -d "$ubi" -a

status=0
for pair in "$@"; do
	name="${pair%%=*}"
	file="${pair#*=}"
	volume=""
	for dir in /sys/class/ubi/ubi"$ubi"_*; do
		if [[ $(< "$dir/name") == "$name" ]]; then
			volume="/dev/$(basename "$dir")"
		fi
	done
	if [[ -z $volume ]]; then
		echo "FAIL: no volume named $name" >&2
		status=1
	elif cmp -n "$(stat -c %s "$file")" "$volume" "$file"; then
		echo "ok: $name ($volume) starts with $file"
	else
		echo "FAIL: $name ($volume) differs from $file" >&2
		status=1
	fi
done
exit "$status"
