# Imported Rootfs Cross AArch64

This example proves Gaia can:

- import a base rootfs tar
- pull local git repos
- cross-build Rust and Go projects for `aarch64`
- install those artifacts into the imported image
- stage runtime content
- verify the final installed binaries are actually `ARM aarch64`

Prep and run:

```bash
./scripts/build-aarch64-cross-artifacts-image.sh
./examples/imported-rootfs-cross-aarch64/scripts/prepare-example.sh
CARGO_TARGET_DIR=.gaia/examples/imported-rootfs-cross-aarch64/local-target cargo run -p gaia -- run examples/imported-rootfs-cross-aarch64/build.local.toml
./examples/imported-rootfs-cross-aarch64/scripts/verify-output.sh
```
