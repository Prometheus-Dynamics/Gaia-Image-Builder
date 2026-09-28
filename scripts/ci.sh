#!/usr/bin/env bash
# Local mirror of .github/workflows/ci.yml (workspace + docs-and-lints jobs).
# Keep the two in sync. Set GAIA_CI_SKIP_E2E=1 to skip the slow end-to-end test
# that compiles the whole Gaia workspace as an image artifact.
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root_dir"

echo "==> Checking formatting"
cargo fmt --check

echo "==> Checking file sizes"
"$root_dir/scripts/check-file-sizes.sh"

echo "==> Checking duplicate dependencies"
cargo tree --workspace --duplicates

echo "==> Running clippy"
cargo clippy --workspace \
  --all-targets \
  --all-features \
  -- -D warnings

echo "==> Checking app without default features"
cargo check -p gaia-app --no-default-features

echo "==> Running tests"
cargo test --workspace --all-features

if [[ "${GAIA_CI_SKIP_E2E:-0}" != "1" ]]; then
  echo "==> Running slow end-to-end tests"
  cargo test -p gaia-exec --test exec_default -- --ignored
fi

echo "==> Building docs"
cargo doc --workspace --no-deps
