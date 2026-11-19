#!/usr/bin/env bash
# qemu-virt-kernel.sh: a board's kernel with the virt machine's devices built
# in, for scripts/qemu-rpi-slots.sh --kernel (a rehearsal of a board image in
# QEMU: the image's own initramfs, root slot and data, with this kernel).
#
#   qemu-virt-kernel.sh DEVICES PRODUCT BOARD OUT [gaia build args...]
#
# DEVICES is a description tree (a copy is made under OUT; the original is
# not written), PRODUCT and BOARD the product and board files under it
# (relative to DEVICES). The copy's board gets scripts/qemu-virt-kernel.config
# as its last kernel fragment, and the product is built until its image's
# `kernel` step, with the store the environment names (GAIA_STORE). Every
# fragment line must take, as in any build: a line the kernel drops fails.
# OUT/Image is the kernel (the build's `kernel` output, Image or Image.gz as
# the board makes it; QEMU takes either), OUT/Image.sha256 its
# digest, OUT/build.log the build. The kernel is the board's own source and
# configuration but for the added lines, so it boots the image's initramfs
# and slot chain as the board would. Pi hardware (RP1, the bcm2835 watchdog,
# the firmware's boot count) is absent on virt.
#
# Needs gaia on PATH ($GAIA names another), python3, and GAIA_STORE set.
set -euo pipefail
[[ $# -ge 4 ]] || { sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$0"; exit 2; }
devices=$(realpath "$1"); product=$2; board=$3; out=$4; shift 4
here=$(dirname "$(realpath "$0")")
gaia=${GAIA:-gaia}
[[ -n ${GAIA_STORE:-} ]] || { echo "GAIA_STORE names the store the kernel is built in" >&2; exit 2; }
[[ -f $devices/$product && -f $devices/$board ]] || { echo "no $product or $board under $devices" >&2; exit 2; }
[[ ! -e $out ]] || { echo "$out exists: give a new directory" >&2; exit 2; }
mkdir -p "$out"
cp -a "$devices" "$out/devices"
cp "$here/qemu-virt-kernel.config" "$(dirname "$out/devices/$board")/qemu-virt-kernel.config"
# The board's fragment list gains the virt fragment, last, so its lines win.
python3 -I - "$out/devices/$board" <<'PY'
import re, sys
path = sys.argv[1]
text = open(path).read()
match = re.search(r'^(fragments\s*=\s*\[)(.*?)(\])', text, re.M | re.S)
if not match:
    sys.exit(f"{path}: no kernel `fragments` list to add the virt fragment to")
items = match.group(2).rstrip().rstrip(",")
added = (items + ", " if items.strip() else "") + '"qemu-virt-kernel.config"'
text = text[: match.start()] + match.group(1) + added + match.group(3) + text[match.end():]
open(path, "w").write(text)
PY
cd "$(dirname "$out/devices/$product")"
name=$(python3 -I -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1],"rb"))["images"][0]["name"])' "$(basename "$product")")
"$gaia" build "$(basename "$product")" --until "$name.kernel" "$@" > "$out/build.log" 2>&1 || {
	echo "the kernel build failed: $out/build.log" >&2; tail -20 "$out/build.log" >&2; exit 1; }
# The kernel: the Buildroot step's `kernel` tree that holds an Image, in the
# run record the build names.
run=$(grep -o 'recorded in [^ ]*\.json' "$out/build.log" | tail -1 | cut -d' ' -f3)
[[ -n $run && -f $run ]] || { echo "the build names no run record: $out/build.log" >&2; exit 1; }
kernel=$(python3 -I - "$run" "$GAIA_STORE" <<'PY'
import json, os, sys
run, store = sys.argv[1], sys.argv[2]
found = []
for step in json.load(open(run))["steps"]:
    tree = (step.get("outputs") or {}).get("kernel", "")
    if step.get("step") != "gaia.os.buildroot" or not tree.startswith("tree:sha256:"):
        continue
    digest = tree.split(":")[-1]
    path = os.path.join(store, "objects", "tree", digest[:2], digest)
    found += [os.path.join(path, f) for f in ("Image", "Image.gz") if os.path.isfile(os.path.join(path, f))]
if len(found) != 1:
    sys.exit(f"{len(found)} kernel images in the run's Buildroot steps: {found}")
print(found[0])
PY
)
cp "$kernel" "$out/Image"
(cd "$out" && sha256sum Image > Image.sha256)
cat "$out/Image.sha256"
