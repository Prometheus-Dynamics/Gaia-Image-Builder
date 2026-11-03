################################################################################
#
# k230-vglite
#
################################################################################

# The SDK's own copy (`make sync` puts it in the Buildroot tree).
K230_VGLITE_SITE = $(TOPDIR)/package/vg_lite
K230_VGLITE_SITE_METHOD = local
K230_VGLITE_LICENSE = MIT
K230_VGLITE_INSTALL_STAGING = YES

define K230_VGLITE_BUILD_CMDS
	$(TARGET_MAKE_ENV) $(MAKE) CC="$(TARGET_CC)" CFLAGS="$(TARGET_CFLAGS)" \
		-C $(@D)/VGLite
endef

define K230_VGLITE_INSTALL_STAGING_CMDS
	$(INSTALL) -D -m 0755 $(@D)/drivers/libvg_lite.so \
		$(STAGING_DIR)/usr/lib/libvg_lite.so
	$(INSTALL) -m 0644 $(@D)/inc/*.h $(STAGING_DIR)/usr/include/
endef

define K230_VGLITE_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/drivers/libvg_lite.so \
		$(TARGET_DIR)/usr/lib/libvg_lite.so
endef

$(eval $(generic-package))
