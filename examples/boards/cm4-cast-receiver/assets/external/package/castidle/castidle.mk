################################################################################
#
# castidle
#
################################################################################

CASTIDLE_VERSION = 1.0
CASTIDLE_SITE = $(CASTIDLE_PKGDIR)/src
CASTIDLE_SITE_METHOD = local
CASTIDLE_LICENSE = GPL-2.0 (Terminus font from the kernel tree)
# The font is read from the kernel source.
CASTIDLE_DEPENDENCIES = linux

# The glyph bytes of lib/fonts/font_ter16x32.c, without its comments (each
# glyph's comment names its code point in hex).
define CASTIDLE_BUILD_CMDS
	sed 's#/\*.*\*/##g' $(LINUX_DIR)/lib/fonts/font_ter16x32.c \
		| grep -o '0x[0-9a-fA-F][0-9a-fA-F]' | sed 's/$$/,/' \
		> $(@D)/font_ter16x32.inc
	$(TARGET_CC) $(TARGET_CFLAGS) $(TARGET_LDFLAGS) -std=gnu11 -Wall -Wextra \
		-I$(@D) -o $(@D)/castidle $(@D)/castidle.c
endef

define CASTIDLE_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/castidle $(TARGET_DIR)/usr/bin/castidle
endef

$(eval $(generic-package))
