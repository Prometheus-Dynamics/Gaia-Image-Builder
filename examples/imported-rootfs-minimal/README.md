# Imported Rootfs Minimal

Fastest imported-base-image example in the repo.

What it covers:
- one local rootfs directory
- one starting-point image definition
- one real tar archive output

Useful commands:
```bash
CARGO_TARGET_DIR=.gaia/examples/imported-rootfs-minimal/local-target cargo run -p gaia -- validate examples/imported-rootfs-minimal/build.toml
CARGO_TARGET_DIR=.gaia/examples/imported-rootfs-minimal/local-target cargo run -p gaia -- plan examples/imported-rootfs-minimal/build.toml
CARGO_TARGET_DIR=.gaia/examples/imported-rootfs-minimal/local-target cargo run -p gaia -- run examples/imported-rootfs-minimal/build.toml
```

Expected output:
- `.gaia/examples/imported-rootfs-minimal/out/images/imported-rootfs-minimal-2.0.0.tar`
