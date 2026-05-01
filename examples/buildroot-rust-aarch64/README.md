# Buildroot Rust AArch64

This example proves the real cross-target Rust path:

- Buildroot emits a real `aarch64` rootfs tar image
- the Rust artifact is built for `aarch64-unknown-linux-gnu`
- the binary is installed into the final image
- one staged OS tweak is also present in the image

Build the repo-owned Docker images first:

```bash
./scripts/build-buildroot-smoke-image.sh
./scripts/build-rust-aarch64-smoke-image.sh
```

Then run the example:

```bash
CARGO_TARGET_DIR=.gaia/examples/buildroot-rust-aarch64/local-target cargo run -p gaia -- run examples/buildroot-rust-aarch64/build.toml
```

Expected primary output:

- `.gaia/examples/buildroot-rust-aarch64/out/images/buildroot-rust-aarch64-2.0.0.tar`

The emitted tar should contain:

- `/usr/bin/smoke-app`
- `/etc/motd`

And the extracted `/usr/bin/smoke-app` should report as an `aarch64` ELF binary.
