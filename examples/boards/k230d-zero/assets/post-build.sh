#!/bin/sh
# Buildroot post-build for the K230D Zero: the SDK's own (Buildroot runs
# post-build scripts from its top dir, the SDK's Buildroot tree), then the
# USB gadget network and an SSH key when the build has one.
set -e
here=$(dirname "$0")
bash board/canaan/k230-soc/post-build.sh "$@"
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
