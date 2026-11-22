#!/usr/bin/env bash
# Builds the stock host for scripts/qemu-rpi-slots.sh --usb-host: an x86_64
# guest that stands for a PC a board is plugged into, with its distribution's
# kernel and drivers as they are. It attaches the board profile's UDC
# (usbip-vudc, over vsock) to vhci_hcd and does what a host does with a
# plugged-in device; the result is its serial log (see
# scripts/qemu-stock-host/init).
#
#   scripts/qemu-stock-host.sh --busybox FILE <out>
#
# <out>/kernel and <out>/initramfs.cpio.gz are made, and <out>/SOURCES says
# where each byte came from (with SHA-256). The initramfs holds a static
# busybox (--busybox: a static x86_64 one; none is built here), the attach
# helper (scripts/qemu-stock-host/vhci-attach.rs, built with rustc for
# x86_64-unknown-linux-musl), the init, and the kernel's own modules for
# USB/IP's host side, CDC NCM, CDC ACM and virtio vsock, decompressed.
#
# Options:
#   --busybox FILE   static x86_64 busybox (required)
#   --kernel FILE    the host kernel (default /boot/vmlinuz-$(uname -r))
#   --modules DIR    its modules (default /lib/modules/<its release>)
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
busybox=""
kernel=/boot/vmlinuz-$(uname -r)
modules=""
out=""
while [[ $# -gt 0 ]]; do
	case $1 in
	--busybox) busybox="$2"; shift 2 ;;
	--kernel) kernel="$2"; shift 2 ;;
	--modules) modules="$2"; shift 2 ;;
	-h | --help) sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$0"; exit 0 ;;
	-*) echo "unknown option $1" >&2; exit 2 ;;
	*) out="$1"; shift ;;
	esac
done
[[ -n $out && -n $busybox ]] || { echo "usage: $0 --busybox FILE <out>" >&2; exit 2; }
[[ -r $busybox ]] || { echo "cannot read --busybox $busybox" >&2; exit 2; }
[[ -r $kernel ]] || { echo "cannot read --kernel $kernel" >&2; exit 2; }
if [[ -z $modules ]]; then
	release=$(file -bL "$kernel" | sed -n 's/.*version \([^ ]*\).*/\1/p')
	[[ -n $release ]] || { echo "$kernel: no release in it; give --modules" >&2; exit 2; }
	modules=/lib/modules/$release
fi
[[ -d $modules ]] || { echo "no modules directory $modules" >&2; exit 2; }
for tool in rustc zstd cpio gzip; do
	command -v "$tool" > /dev/null || { echo "missing $tool" >&2; exit 2; }
done

# Load order: each after what it depends on.
order=(mii usbnet cdc_ether cdc_ncm cdc-acm usbip-core vhci-hcd vsock vmw_vsock_virtio_transport_common vmw_vsock_virtio_transport)

mkdir -p "$out"
root=$(mktemp -d "${TMPDIR:-/tmp}/stock-host.XXXXXX")
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/bin" "$root/lib/modules" "$root/dev"
cp "$busybox" "$root/bin/busybox"
chmod 755 "$root/bin/busybox"
cp "$here/qemu-stock-host/init" "$root/init"
chmod 755 "$root/init"
rustc --edition 2021 -O --target x86_64-unknown-linux-musl -C panic=abort -C strip=symbols \
	-o "$root/bin/vhci-attach" "$here/qemu-stock-host/vhci-attach.rs"
{
	echo "kernel $kernel sha256 $(sha256sum < "$kernel" | cut -d' ' -f1)"
	echo "busybox $busybox sha256 $(sha256sum < "$busybox" | cut -d' ' -f1)"
	echo "vhci-attach from scripts/qemu-stock-host/vhci-attach.rs sha256 $(sha256sum < "$here/qemu-stock-host/vhci-attach.rs" | cut -d' ' -f1) ($(rustc --version))"
	echo "init scripts/qemu-stock-host/init sha256 $(sha256sum < "$here/qemu-stock-host/init" | cut -d' ' -f1)"
} > "$out/SOURCES"
for m in "${order[@]}"; do
	file=$(find "$modules/kernel" -name "$m.ko" -o -name "$m.ko.zst" -o -name "$m.ko.xz" -o -name "$m.ko.gz" | head -n1)
	[[ -n $file ]] || { echo "no module $m under $modules" >&2; exit 1; }
	case $file in
	*.zst) zstd -dq -c "$file" > "$root/lib/modules/$m.ko" ;;
	*.xz) xz -dc "$file" > "$root/lib/modules/$m.ko" ;;
	*.gz) gzip -dc "$file" > "$root/lib/modules/$m.ko" ;;
	*) cp "$file" "$root/lib/modules/$m.ko" ;;
	esac
	echo "module $file sha256 $(sha256sum < "$file" | cut -d' ' -f1)" >> "$out/SOURCES"
	echo "$m" >> "$root/lib/modules/order"
done
(cd "$root" && find . -mindepth 1 | LC_ALL=C sort | cpio --quiet -o -H newc -R 0:0) | gzip -9n > "$out/initramfs.cpio.gz"
cp "$kernel" "$out/kernel"
echo "stock host in $out:"
cat "$out/SOURCES"
