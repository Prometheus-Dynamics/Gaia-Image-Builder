#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image_name="gaia-starting-point-raw-smoke:latest"

docker build -t "$image_name" -f "$repo_root/docker/starting-point-raw-smoke/Dockerfile" "$repo_root"

docker run --rm --privileged \
  -v "$repo_root:/workspace" \
  -w /workspace \
  "$image_name" \
  bash -lc '
    set -euo pipefail
    ./examples/imported-raw-image-mutate/scripts/prepare-base-image.sh
    CARGO_TARGET_DIR=/workspace/.gaia/examples/imported-raw-image-mutate/local-target /usr/local/cargo/bin/cargo run -q -p gaia -- run examples/imported-raw-image-mutate/build.toml
    ./examples/imported-raw-image-mutate/scripts/verify-final-image.sh
  '
