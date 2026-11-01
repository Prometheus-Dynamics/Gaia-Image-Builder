#!/bin/sh
# Post-build: trim the target for a read-only appliance.
set -e
TARGET="$1"

# Mount points the overlay's fstab uses.
mkdir -p "$TARGET/boot"

# No telnet, cron or web server from BusyBox's defaults, no ifplugd.
rm -f "$TARGET/etc/init.d/S50telnet" "$TARGET/etc/init.d/S50crond" \
	"$TARGET/etc/init.d/S90httpd" "$TARGET/etc/init.d/S41ifplugd" \
	"$TARGET/etc/init.d/S15watchdog"

# Translations, headers and docs.
rm -rf "$TARGET/usr/share/locale" "$TARGET/usr/share/doc" \
	"$TARGET/usr/share/man" "$TARGET/usr/share/gtk-doc" \
	"$TARGET/usr/include" "$TARGET/usr/share/gstreamer-1.0/presets"

# Wi-Fi firmware: the CM4's (CYW43455, or CYW43456 on later modules); no
# Pi 3, Pi 4 B, Pi 400 or Pi 5 board files.
fw="$TARGET/lib/firmware/brcm"
if [ -d "$fw" ]; then
	find "$fw" -maxdepth 1 \( -type f -o -type l \) \
		! -name 'brcmfmac4345[56]-sdio.bin' \
		! -name 'brcmfmac4345[56]-sdio.clm_blob' \
		! -name 'brcmfmac4345[56]-sdio.txt' \
		! -name 'brcmfmac4345[56]-sdio.raspberrypi,4-compute-module.*' \
		-delete
fi

# avahi's autoipd setup (link-local addresses are not used).
rm -f "$TARGET/etc/init.d/S05avahi-setup.sh"
