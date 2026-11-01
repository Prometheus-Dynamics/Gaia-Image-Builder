#!/bin/sh
# Post-build: trim the target for a read-only appliance, and record the
# board's settings. Arguments: the target dir, then the board (cm4, pi5;
# BR2_ROOTFS_POST_SCRIPT_ARGS).
set -e
TARGET="$1"
BOARD="$2"

case "$BOARD" in
cm4)
	# The VideoCore H.264 decoder, its frames handed to kmssink as they
	# are (dmabuf, no conversion); -bt709 is the colorimetry fix UxPlay
	# needs for it. The CM4 has a CYW43455 (CYW43456 on later modules).
	video="-v4l2 -vc identity -bt709"
	wifi='brcmfmac4345[56]-sdio.raspberrypi,4-compute-module.*'
	;;
pi5)
	# No H.264 block on the BCM2712: FFmpeg's decoder on the A76s, its
	# I420 frames to kmssink without a conversion. CYW43455.
	video="-avdec -vc identity"
	wifi='brcmfmac43455-sdio.raspberrypi,5*'
	;;
*)
	echo "post-build.sh: unknown board '$BOARD' (BR2_ROOTFS_POST_SCRIPT_ARGS)" >&2
	exit 1
	;;
esac
mkdir -p "$TARGET/etc/cast"
printf 'BOARD=%s\nUXPLAY_VIDEO="%s"\n' "$BOARD" "$video" > "$TARGET/etc/cast/board.env"

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

# Wi-Fi firmware: the board's chip and board files only.
fw="$TARGET/lib/firmware/brcm"
if [ -d "$fw" ]; then
	find "$fw" -maxdepth 1 \( -type f -o -type l \) \
		! -name 'brcmfmac4345[56]-sdio.bin' \
		! -name 'brcmfmac4345[56]-sdio.clm_blob' \
		! -name 'brcmfmac4345[56]-sdio.txt' \
		! -name "$wifi" \
		-delete
fi

# avahi's autoipd setup (link-local addresses are not used).
rm -f "$TARGET/etc/init.d/S05avahi-setup.sh"
