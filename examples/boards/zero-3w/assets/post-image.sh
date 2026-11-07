#!/bin/sh
# Buildroot post-image: the maskrom loader (rkbin's boot_merger output, for
# rkdeveloptool and Atlas) next to the images. Buildroot passes the images
# dir first; then the RKBOOT ini and, optionally, the rkbin tree (default:
# Buildroot's rockchip-rkbin build dir). boot_merger runs in a scratch dir,
# so the rkbin tree is only read.
set -e
ini="${2:?usage: post-image.sh <images dir> <RKBOOT ini> [rkbin dir]}"
rkbin="${3:-$(ls -d "$BUILD_DIR"/rockchip-rkbin-* | head -n 1)}"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
ln -s "$rkbin/bin" "$work/bin"
mkdir "$work/RKBOOT"
cp "$rkbin/RKBOOT/$ini" "$work/RKBOOT/$ini"
(cd "$work" && "$rkbin/tools/boot_merger" "RKBOOT/$ini")
loader=$(sed -n 's/^PATH=//p' "$rkbin/RKBOOT/$ini" | tr -d '\r')
install -m 0644 "$work/$loader" "$BINARIES_DIR/$loader"
# NEWIDB=true inis also make an idblock (IDB_PATH=, else idblock.img): the
# loader's DDR init + SPL laid out for the boot media, kept beside it.
idb=$(sed -n 's/^IDB_PATH=//p' "$rkbin/RKBOOT/$ini" | tr -d '\r')
idb="${idb:-idblock.img}"
if [ -f "$work/$idb" ]; then
    install -m 0644 "$work/$idb" "$BINARIES_DIR/$idb"
fi
# rkbin's licence beside the loader and the images that carry its blobs.
install -m 0644 "$rkbin/LICENSE" "$BINARIES_DIR/rkbin-LICENSE.txt"
