#!/bin/sh
# Buildroot post-build: the extlinux.conf U-Boot's distro boot reads from
# the bootable rootfs-a partition, rkbin's licence (the image carries its DDR
# init), the USB gadget network, and an SSH key when the build has one.
set -e
here=$(dirname "$0")
install -m 0644 -D "$here/extlinux.conf" "$TARGET_DIR/boot/extlinux/extlinux.conf"
rkbin="${3:-$(ls -d "$BUILD_DIR"/rockchip-rkbin-* | head -n 1)}"
install -m 0644 -D "$rkbin/LICENSE" "$TARGET_DIR/usr/share/licenses/rkbin/LICENSE"
# usb0: 10.55.0.1/24, and a DHCP server that gives the host 10.55.0.2.
if ! grep -q '^auto usb0' "$TARGET_DIR/etc/network/interfaces"; then
	cat >> "$TARGET_DIR/etc/network/interfaces" <<'IFACE'

auto usb0
iface usb0 inet static
  address 10.55.0.1
  netmask 255.255.255.0
  post-up udhcpd /etc/udhcpd.usb0.conf
IFACE
fi
cat > "$TARGET_DIR/etc/udhcpd.usb0.conf" <<'DHCP'
interface usb0
start 10.55.0.2
end 10.55.0.2
max_leases 1
lease_file /tmp/udhcpd.usb0.leases
option subnet 255.255.255.0
DHCP
# Optional: assets/authorized_keys (not in the repo) for root's SSH login.
if [ -f "$here/authorized_keys" ]; then
	install -m 0600 -D "$here/authorized_keys" "$TARGET_DIR/root/.ssh/authorized_keys"
fi
