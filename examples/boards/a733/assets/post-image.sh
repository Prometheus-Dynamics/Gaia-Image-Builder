#!/bin/sh
# Buildroot post-image for the A733 boards: the SPL must carry the DRAM init
# blob it was given, unchanged and once. The blob is closed code with no
# redistribution grant: the SPL and every image containing it stay private.
set -e
blob_sha256=d89804f404f2eef6ef5b03d82a310ef0906c4db8ded760be221b131f5c813a05
blob=$(sed -n 's/^BR2_TARGET_UBOOT_CUSTOM_DTS_PATH="\(.*\)"$/\1/p' "$BR2_CONFIG")
spl="$BINARIES_DIR/u-boot-sunxi-with-spl.bin"
echo "$blob_sha256  $blob" | sha256sum -c -
python3 -I - "$spl" "$blob" <<'PY'
import sys
spl = open(sys.argv[1], "rb").read()
blob = open(sys.argv[2], "rb").read()
count = spl.count(blob)
if count != 1:
    sys.exit(f"{sys.argv[1]} carries the DRAM blob {count} times, expected once")
print(f"the SPL carries the DRAM blob unchanged at offset {spl.find(blob):#x}")
PY
