# Buildroot Rust Squashfs

Runnable Buildroot smoke example that emits a real non-`tar` image artifact.

What it covers:
- one `rust` artifact
- one install mapping
- one staged OS file
- one Buildroot image
- one final `squashfs` root filesystem artifact

Useful commands:
```bash
CARGO_TARGET_DIR=.gaia/examples/buildroot-rust-squashfs/local-target cargo run -p gaia -- validate examples/buildroot-rust-squashfs/build.toml
CARGO_TARGET_DIR=.gaia/examples/buildroot-rust-squashfs/local-target cargo run -p gaia -- plan examples/buildroot-rust-squashfs/build.toml
CARGO_TARGET_DIR=.gaia/examples/buildroot-rust-squashfs/local-target cargo run -p gaia -- run examples/buildroot-rust-squashfs/build.toml
```

This example exists to prove Gaia updates final non-`tar` Buildroot outputs after applying
Gaia-managed installs and staged OS changes.
