#!/usr/bin/env bash
# Makes the shared artifact cache (see `acache --help`) from nothing, or
# brings an existing one's tool up to this checkout's: the root, its v1
# directory, and bin/acache copied from here. Entries are never touched.
#
#   scripts/acache/setup.sh [ROOT]    ROOT: $ACACHE_ROOT, else
#                                      /mnt/nvme-wd/artifact-cache
#
# The copy is written beside the old one and renamed over it, so a
# process running the old tool keeps reading a whole file.
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=${1:-${ACACHE_ROOT:-/mnt/nvme-wd/artifact-cache}}
mkdir -p "$root/bin" "$root/v1"
tmp=$(mktemp "$root/bin/.acache.XXXXXX")
cp "$here/acache" "$tmp"
chmod 755 "$tmp"
mv -f "$tmp" "$root/bin/acache"
echo "acache: $root/bin/acache ($(sha256sum "$root/bin/acache" | cut -c1-12)), entries in $root/v1"
