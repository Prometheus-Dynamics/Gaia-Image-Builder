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

echo "==> Signing tools (the real sign-and-verify tests skip without them)"
for tool in minisign cosign; do
  if command -v "$tool" >/dev/null 2>&1; then
    echo "    $tool: $(command -v "$tool")"
  else
    echo "    $tool: not on PATH; its real sign-and-verify tests are SKIPPED"
  fi
done

echo "==> Testing the QEMU runner's firmware emulation"
"$root_dir/scripts/qemu-rpi-slots-test.sh"

echo "==> Running tests"
# nextest runs every test in parallel (~15 s here against ~60 s for cargo
# test); cargo test stays the fallback where it is not installed.
if cargo nextest --version >/dev/null 2>&1; then
  cargo nextest run --workspace --all-features --profile "${GAIA_NEXTEST_PROFILE:-default}"
  cargo test --workspace --all-features --doc
else
  cargo test --workspace --all-features
fi

if [[ "${GAIA_CI_SKIP_E2E:-0}" != "1" ]]; then
  echo "==> Running slow end-to-end tests"
  cargo test -p gaia-exec --test exec_default -- --ignored
  echo "==> Loading and running the OCI outputs (skopeo, podman, docker; each skipped when missing)"
  cargo test -p gaia-exec --lib tests_oci -- --ignored --nocapture
fi

echo "==> Building docs"
cargo doc --workspace --no-deps

echo "==> Validating package surface"
cargo package --workspace --allow-dirty --no-verify
