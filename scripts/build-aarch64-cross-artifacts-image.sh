#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

docker build \
  -f "$repo_root/docker/aarch64-cross-artifacts/Dockerfile" \
  -t gaia-aarch64-cross-artifacts:latest \
  "$repo_root"
