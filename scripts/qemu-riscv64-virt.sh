#!/usr/bin/env bash
# Boots a riscv64 kernel and initramfs under qemu-system-riscv64 -M virt,
# with OpenSBI as the firmware, and checks that the boot marker appears on
# the serial console. This is the riscv64 qemu tier, the counterpart of
# scripts/qemu-rpi-slots.sh for the K230 family. It is NOT K230 hardware:
# the virt machine has no K230 SoC, device tree or boot ROM, and a pass says
# only that a riscv64 kernel of this config family boots and runs userspace.
#
#   scripts/qemu-riscv64-virt.sh --kernel Image --initramfs initramfs.cpio
#   scripts/qemu-riscv64-virt.sh --kernel Image --root <root tree> --marker 'login:'
#
# --root boots an image's own root filesystem tree (the build's merged root)
# as the initramfs, so the image's init, its services and its login prompt
# run: the tree is packed as it is, owned by root, with one file added, an
# /init that mounts devtmpfs on /dev and runs /sbin/init (a root booted from
# a disk gets /dev from the kernel; an initramfs does not). Give the marker
# the image prints, its login prompt say.
#
# --cpu is QEMU's CPU (default rv64,v=true,vlen=128): the K230's cores have
# the vector extension with 128-bit registers, and the family's userspace is
# built for it (BR2_RISCV_ISA_RVV=y). On a CPU without it the image's first
# program dies of an illegal instruction.
#
# --print shows the QEMU command and boots nothing.
#
# Exit codes: 0 marker seen; 1 marker missing or the guest died first; 2
# usage or a missing tool. The qemu process is started by this script, and
# only its own PID is stopped (on timeout, or when the marker is seen).
set -u

kernel=""
initramfs=""
root=""
print=0
timeout_s=120
cpu="rv64,v=true,vlen=128"
marker="GAIA-QEMU-RISCV64-VIRT BOOT OK"
bios="${QEMU_RISCV64_BIOS:-/usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin}"

usage() {
	echo "usage: $0 --kernel <Image> [--initramfs <cpio> | --root <tree>] [--cpu <cpu>] [--timeout <s>] [--marker <text>] [--print]" >&2
	exit 2
}

while [ $# -gt 0 ]; do
	case "$1" in
	--kernel) kernel=${2:?}; shift 2 ;;
	--initramfs) initramfs=${2:?}; shift 2 ;;
	--root) root=${2:?}; shift 2 ;;
	--cpu) cpu=${2:?}; shift 2 ;;
	--timeout) timeout_s=${2:?}; shift 2 ;;
	--marker) marker=${2:?}; shift 2 ;;
	--print) print=1; shift ;;
	*) usage ;;
	esac
done
[ -n "$kernel" ] || usage
if [ -n "$root" ] && [ -n "$initramfs" ]; then
	echo "--root and --initramfs: one of them (--root makes the initramfs from the tree)" >&2
	exit 2
fi

tools=(qemu-system-riscv64)
[ -z "$root" ] || tools+=(cpio)
for tool in "${tools[@]}"; do
	command -v "$tool" >/dev/null 2>&1 || { echo "missing tool: $tool" >&2; exit 2; }
done
[ -r "$kernel" ] || { echo "no kernel: $kernel" >&2; exit 2; }
[ -r "$bios" ] || { echo "no OpenSBI firmware: $bios (set QEMU_RISCV64_BIOS)" >&2; exit 2; }
if [ -n "$root" ]; then
	[ -d "$root" ] || { echo "no root tree: $root" >&2; exit 2; }
	[ -e "$root/sbin/init" ] || [ -L "$root/sbin/init" ] || { echo "$root has no /sbin/init: not a root filesystem tree" >&2; exit 2; }
fi

work=""
cleanup() { [ -z "$work" ] || { chmod -R u+w "$work" 2>/dev/null; rm -rf "$work"; }; }
trap cleanup EXIT

args=(-M virt -cpu "$cpu" -m 256M -smp 2 -nographic -no-reboot
	-bios "$bios" -kernel "$kernel"
	-append "console=ttyS0 panic=-1")
if [ -n "$root" ] && [ "$print" = 1 ]; then
	args+=(-initrd "<initramfs of $root>")
elif [ -n "$root" ]; then
	work=$(mktemp -d "${TMPDIR:-/tmp}/qemu-riscv64-root.XXXXXX")
	cp -a "$root" "$work/root"
	chmod -R u+w "$work/root"
	rm -f "$work/root/init"
	printf '#!/bin/sh\n/bin/mount -t devtmpfs devtmpfs /dev\nexec /sbin/init\n' > "$work/root/init"
	chmod 755 "$work/root/init"
	(cd "$work/root" && find . | LC_ALL=C sort | cpio -o -H newc -R 0:0 --quiet) > "$work/initramfs.cpio" ||
		{ echo "packing $root failed" >&2; exit 2; }
	initramfs="$work/initramfs.cpio"
	args+=(-initrd "$initramfs")
elif [ -n "$initramfs" ]; then
	[ -r "$initramfs" ] || { echo "no initramfs: $initramfs" >&2; exit 2; }
	args+=(-initrd "$initramfs")
fi

if [ "$print" = 1 ]; then
	printf 'qemu-system-riscv64'
	printf ' %q' "${args[@]}"
	printf '\n'
	exit 0
fi

log=$(mktemp "${TMPDIR:-/tmp}/qemu-riscv64-virt.XXXXXX")
echo "NOT K230 HARDWARE: qemu riscv64 virt, cpu=$cpu kernel=$kernel initramfs=${root:+the root tree $root}${root:-${initramfs:-none}}"
qemu-system-riscv64 "${args[@]}" < /dev/null >"$log" 2>&1 &
qpid=$!

seen=0
for _ in $(seq "$timeout_s"); do
	if grep -aq -- "$marker" "$log" 2>/dev/null; then seen=1; break; fi
	kill -0 "$qpid" 2>/dev/null || break
	sleep 1
done
# The marker may be the last thing printed before the guest ended.
[ "$seen" = 1 ] || { grep -aq -- "$marker" "$log" 2>/dev/null && seen=1; }
if kill -0 "$qpid" 2>/dev/null; then kill "$qpid" 2>/dev/null; wait "$qpid" 2>/dev/null; fi

cat "$log"
rm -f "$log"
if [ "$seen" = 1 ]; then
	echo "RESULT: marker seen (not K230 hardware)"
	exit 0
fi
echo "RESULT: marker '$marker' not seen within ${timeout_s}s (not K230 hardware)" >&2
exit 1
