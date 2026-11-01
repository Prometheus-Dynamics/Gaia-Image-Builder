# Buildroot 2024.02's host tools (m4, bison and others) predate C23, the
# default of GCC 15 and later. Included after package/Makefile.in, and
# HOST_CFLAGS is expanded only when a recipe runs, so every host package
# gets it.
HOST_CFLAGS += -std=gnu17
