#!/usr/bin/env bash
# Tests scripts/qemu-rpi-slots.sh's firmware emulation (--print: no QEMU)
# on a small MBR disk whose FAT holds a slots layout like a Raze's.
# Needs sfdisk, mkfs.fat, mtools and dtc's fdtput.
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runner="$root_dir/scripts/qemu-rpi-slots.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# No default SSH key from the caller's home.
export HOME="$work"

disk="$work/disk.img"
truncate -s 40M "$disk"
printf 'label: dos\nlabel-id: 0x5056525a\nstart=2048, size=61440, type=c, bootable\nstart=63488, type=83\n' |
	sfdisk -q "$disk"
mkfs.fat -F 16 --offset 2048 "$disk" 30720 > /dev/null 2>&1
fat="$disk@@1M"

put() { printf '%b' "$2" | mcopy -o -i "$fat" - "::$1"; }
mmd -i "$fat" ::slots ::slots/a ::slots/b
put config.txt 'arm_64bit=1\n[cm4]\nos_prefix=slots/wrong/\n[all]\ninitramfs initramfs followkernel\ninclude default.txt\n'
put tryboot.txt 'arm_64bit=1\n[all]\ninitramfs initramfs followkernel\ninclude trial.txt\n'
put default.txt 'os_prefix=slots/a/\n'
put trial.txt '# the trial slot\r\nos_prefix=slots/b/\r\n'
for slot in a b; do
	put "slots/$slot/kernel8.img" "kernel $slot\n"
	put "slots/$slot/initramfs" "initramfs $slot\n"
	put "slots/$slot/cmdline.txt" "console=serial0,115200 console=tty1 rootwait atlas.slot=$slot panic=5\n"
done
put slots/b/kernel_2712.img 'kernel 2712 b\n'
put kernel8.img 'kernel root\n'
put initramfs 'initramfs root\n'
put cmdline.txt 'console=serial0,115200 root=/dev/mmcblk0p2\n'

failures=0
check() {
	local label=$1 want=$2 got=$3
	if [[ $got != *"$want"* ]]; then
		echo "FAIL $label: wanted '$want' in:" >&2
		printf '%s\n' "$got" >&2
		failures=$((failures + 1))
	fi
}

out=$("$runner" --state "$work/st" --print "$disk")
check "default slot" "from: config.txt: slots/a/kernel8.img, slots/a/initramfs, slots/a/cmdline.txt" "$out"
check "cmdline" "cmdline: console=ttyAMA0,115200 rootwait atlas.slot=a panic=5 net.ifnames=0 atlas.tryboot=0" "$out"
check "tryboot flag" "tryboot: 0" "$out"
check "kernel copied" "kernel a" "$(cat "$work/st/boot/kernel")"
check "initramfs copied" "initramfs a" "$(cat "$work/st/boot/initramfs")"
check "tap by default" "tap\\,id=net0\\,ifname=qtap0" "$out"
check "fixed mac" "mac=52:54:00:77:00:02" "$out"
out=$("$runner" --state "$work/st" --net user --print "$disk")
check "user-mode forwards" "hostfwd=tcp::15800-:5800\\,hostfwd=tcp::15899-:5899\\,hostfwd=tcp::19200-:9200\\,hostfwd=tcp::2222-:22" "$out"

out=$("$runner" --state "$work/st" --trial-append systemd.mask=x.service --print "$disk")
if [[ $out == *systemd.mask=x.service* ]]; then
	echo "FAIL: --trial-append reached a default boot" >&2
	failures=$((failures + 1))
fi

out=$("$runner" --state "$work/st" --tryboot --trial-append systemd.mask=x.service --print "$disk")
check "trial append" "atlas.tryboot=1 systemd.mask=x.service" "$out"
check "trial slot" "from: tryboot.txt: slots/b/kernel_2712.img, slots/b/initramfs, slots/b/cmdline.txt" "$out"
check "tryboot flag on" "tryboot: 1" "$out"
check "kernel_2712 preferred" "kernel 2712 b" "$(cat "$work/st/boot/kernel")"
[[ ! -e $work/st/tryboot-next ]] || { echo "FAIL: --tryboot outlived its boot" >&2; failures=$((failures + 1)); }

# The guest's writes land in the copy, never in the image.
put default.txt 'os_prefix=slots/b/\n'
out=$("$runner" --state "$work/st" --print "$disk")
check "copy kept" "slots/a/" "$out"
out=$("$runner" --state "$work/st" --fresh --print "$disk")
check "fresh copy" "slots/b/" "$out"

out=$("$runner" --state "$work/st" --kernel /some/Image --print "$disk" )
check "kernel override" "-kernel /some/Image" "$out"

# A tryboot.req the guest left asks for a tryboot.
put tryboot.req ''
out=$("$runner" --state "$work/st" --fresh --print "$disk")
check "tryboot.req" "from: tryboot.txt: slots/b/" "$out"
check "atlas.tryboot" "atlas.tryboot=1" "$out"
mdel -i "$fat" ::tryboot.req

# An SSH key lands on a new copy's FAT, and only on a new copy.
printf 'ssh-ed25519 AAAAtest qemu\n' > "$work/key.pub"
"$runner" --state "$work/st" --fresh --ssh-key "$work/key.pub" --print "$disk" > /dev/null 2>&1
check "ssh key" "ssh-ed25519 AAAAtest qemu" "$(mtype -i "$work/st/disk.img@@1M" ::board/authorized_keys)"
if mtype -i "$fat" ::board/authorized_keys > /dev/null 2>&1; then
	echo "FAIL: the key was written into the image" >&2
	failures=$((failures + 1))
fi

# A [tryboot] section and an include that opens a section carry on as text.
put config.txt 'include extra.txt\nos_prefix=slots/wrong/\n[all]\ninitramfs initramfs\n[tryboot]\nos_prefix=slots/b/\n'
put extra.txt '[none]\n'
out=$("$runner" --state "$work/st" --fresh --print "$disk")
check "section from an include" "from: config.txt: kernel8.img" "$out"

# A slot file that is missing is named.
put config.txt 'os_prefix=slots/gone/\n'
if out=$("$runner" --state "$work/st" --fresh --print "$disk" 2>&1); then
	echo "FAIL: a missing slot booted" >&2
	failures=$((failures + 1))
fi
check "missing slot named" "config.txt names slots/gone/kernel8.img, which the boot partition lacks" "$out"

if [[ $failures -ne 0 ]]; then
	echo "$failures qemu-rpi-slots check(s) failed" >&2
	exit 1
fi
echo "qemu-rpi-slots: all checks passed"
