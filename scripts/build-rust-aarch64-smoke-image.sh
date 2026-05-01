#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

docker build \
  -f "$repo_root/docker/rust-aarch64-smoke/Dockerfile" \
  -t gaia-rust-aarch64-smoke:latest \
  "$repo_root"
