#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

docker build \
  -t gaia-polyglot-artifacts:latest \
  -f "$repo_root/docker/polyglot-artifacts/Dockerfile" \
  "$repo_root"
