# Buildroot Rust Sdcard

Runnable Buildroot smoke example that emits a real `sdcard.img` artifact.

What it covers:
- one `rust` artifact
- one install mapping
- one staged OS file
- one Buildroot image
- one final `sdcard.img` image generated through typed Gaia assembly

Useful commands:
```bash
CARGO_TARGET_DIR=.gaia/examples/buildroot-rust-sdcard/local-target cargo run -p gaia -- validate examples/buildroot-rust-sdcard/build.toml
CARGO_TARGET_DIR=.gaia/examples/buildroot-rust-sdcard/local-target cargo run -p gaia -- plan examples/buildroot-rust-sdcard/build.toml
CARGO_TARGET_DIR=.gaia/examples/buildroot-rust-sdcard/local-target cargo run -p gaia -- run examples/buildroot-rust-sdcard/build.toml
```

Docker-backed Buildroot smoke:
```bash
./scripts/run-buildroot-sdcard-smoke.sh
```

This example exists to prove Gaia can drive a non-`tar` disk-image output path and promote
the final image artifact as the primary output.
