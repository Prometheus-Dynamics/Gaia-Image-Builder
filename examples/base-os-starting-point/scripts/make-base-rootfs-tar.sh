#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
SEED_DIR="${ROOT_DIR}/inputs/seed-rootfs"
OUT_TAR="${ROOT_DIR}/inputs/base-rootfs.tar"

if [[ ! -d "${SEED_DIR}" ]]; then
  echo "seed rootfs dir not found: ${SEED_DIR}" >&2
  exit 1
fi

rm -f "${OUT_TAR}"
tar -cf "${OUT_TAR}" -C "${SEED_DIR}" .
echo "wrote ${OUT_TAR}"
