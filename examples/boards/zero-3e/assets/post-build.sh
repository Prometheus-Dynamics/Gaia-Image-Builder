#!/bin/sh
# ZERO 3E post-build, after the RK35xx one: usb0 at 10.55.0.1/24 on the
# OTG port, and a DHCP server that gives the host 10.55.0.2.
set -e
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
