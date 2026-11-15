#!/usr/bin/env bash
# Boots a riscv64 kernel and initramfs under qemu-system-riscv64 -M virt,
# with OpenSBI as the firmware, and checks that the boot marker appears on
# the serial console. This is the riscv64 qemu tier, the counterpart of
# scripts/qemu-rpi-slots.sh for the K230 family. It is NOT K230 hardware:
# the virt machine has no K230 SoC, device tree or boot ROM, and a pass says
# only that a riscv64 kernel of this config family boots and runs userspace.
#
#   scripts/qemu-riscv64-virt.sh --kernel Image --initramfs initramfs.cpio
#
# Exit codes: 0 marker seen; 1 marker missing or the guest died first; 2
# usage or a missing tool. The qemu process is started by this script, and
# only its own PID is stopped (on timeout, or when the marker is seen).
set -u

kernel=""
initramfs=""
timeout_s=120
marker="GAIA-QEMU-RISCV64-VIRT BOOT OK"
bios="${QEMU_RISCV64_BIOS:-/usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin}"

usage() {
	echo "usage: $0 --kernel <Image> [--initramfs <cpio>] [--timeout <s>] [--marker <text>]" >&2
	exit 2
}

while [ $# -gt 0 ]; do
	case "$1" in
	--kernel) kernel=${2:?}; shift 2 ;;
	--initramfs) initramfs=${2:?}; shift 2 ;;
	--timeout) timeout_s=${2:?}; shift 2 ;;
	--marker) marker=${2:?}; shift 2 ;;
	*) usage ;;
	esac
done
[ -n "$kernel" ] || usage

for tool in qemu-system-riscv64; do
	command -v "$tool" >/dev/null 2>&1 || { echo "missing tool: $tool" >&2; exit 2; }
done
[ -r "$kernel" ] || { echo "no kernel: $kernel" >&2; exit 2; }
[ -r "$bios" ] || { echo "no OpenSBI firmware: $bios (set QEMU_RISCV64_BIOS)" >&2; exit 2; }

args=(-M virt -cpu rv64 -m 256M -smp 2 -nographic -no-reboot
	-bios "$bios" -kernel "$kernel"
	-append "console=ttyS0 panic=-1")
if [ -n "$initramfs" ]; then
	[ -r "$initramfs" ] || { echo "no initramfs: $initramfs" >&2; exit 2; }
	args+=(-initrd "$initramfs")
fi

log=$(mktemp "${TMPDIR:-/tmp}/qemu-riscv64-virt.XXXXXX")
echo "NOT K230 HARDWARE: qemu riscv64 virt, kernel=$kernel initramfs=${initramfs:-none}"
qemu-system-riscv64 "${args[@]}" >"$log" 2>&1 &
qpid=$!

seen=0
for _ in $(seq "$timeout_s"); do
	if grep -q "$marker" "$log" 2>/dev/null; then seen=1; break; fi
	kill -0 "$qpid" 2>/dev/null || break
	sleep 1
done
if kill -0 "$qpid" 2>/dev/null; then kill "$qpid" 2>/dev/null; wait "$qpid" 2>/dev/null; fi

cat "$log"
if [ "$seen" = 1 ]; then
	echo "RESULT: marker seen (not K230 hardware)"
	rm -f "$log"
	exit 0
fi
echo "RESULT: marker '$marker' not seen within ${timeout_s}s (not K230 hardware)" >&2
rm -f "$log"
exit 1
