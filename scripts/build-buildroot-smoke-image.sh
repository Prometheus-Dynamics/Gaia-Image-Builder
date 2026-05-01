#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image_tag="${1:-gaia-buildroot-smoke:latest}"

docker build \
  -f "$repo_root/docker/buildroot-smoke/Dockerfile" \
  -t "$image_tag" \
  "$repo_root"
