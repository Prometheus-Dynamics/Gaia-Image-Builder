# Buildroot Rust Minimal

Fastest runnable Buildroot example in the repo.

What it covers:
- one `rust` artifact
- one install mapping
- one staged OS file
- one Buildroot image

Useful commands:
```bash
CARGO_TARGET_DIR=.gaia/examples/buildroot-rust-minimal/local-target cargo run -p gaia -- validate examples/buildroot-rust-minimal/build.toml
CARGO_TARGET_DIR=.gaia/examples/buildroot-rust-minimal/local-target cargo run -p gaia -- plan examples/buildroot-rust-minimal/build.toml
CARGO_TARGET_DIR=.gaia/examples/buildroot-rust-minimal/local-target cargo run -p gaia -- run examples/buildroot-rust-minimal/build.toml
```

Docker-backed Buildroot smoke:
```bash
./scripts/build-buildroot-smoke-image.sh
CARGO_TARGET_DIR=.gaia/examples/buildroot-rust-minimal/local-target cargo run -p gaia -- run examples/buildroot-rust-minimal/build.toml \
  --set execution.docker.enabled=true \
  --set execution.docker.image=gaia-buildroot-smoke:latest
```

Why this exists:
- `qemu_x86_64_defconfig`
- no external tree
- one binary
- one simple `/etc/motd` modification
