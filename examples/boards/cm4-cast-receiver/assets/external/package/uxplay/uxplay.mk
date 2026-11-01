################################################################################
#
# uxplay
#
################################################################################

UXPLAY_VERSION = 1.73.7
UXPLAY_SITE = $(call github,FDH2,UxPlay,v$(UXPLAY_VERSION))
UXPLAY_LICENSE = GPL-3.0+
UXPLAY_LICENSE_FILES = LICENSE
UXPLAY_DEPENDENCIES = \
	host-pkgconf \
	openssl \
	libplist \
	avahi \
	dbus \
	gstreamer1 \
	gst1-plugins-base

# -march=native is only ever added on x86 hosts; set it off anyway.
UXPLAY_CONF_OPTS = \
	-DNO_MARCH_NATIVE=ON \
	-DNO_X11_DEPS=ON

$(eval $(cmake-package))
