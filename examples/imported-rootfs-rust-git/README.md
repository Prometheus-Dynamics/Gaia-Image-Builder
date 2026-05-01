# Imported Rootfs Rust Git

This example proves Gaia can:

- import a base rootfs tar through a normal Gaia source
- pull a Rust project from a git repo source
- build that project as an artifact
- install the artifact into the image
- stage files, env, and service content into the final image

Prep and run:

```bash
./examples/imported-rootfs-rust-git/scripts/prepare-example.sh
CARGO_TARGET_DIR=.gaia/examples/imported-rootfs-rust-git/local-target cargo run -p gaia -- run examples/imported-rootfs-rust-git/build.toml
./examples/imported-rootfs-rust-git/scripts/verify-output.sh
```

Expected output:
- `.gaia/examples/imported-rootfs-rust-git/out/images/imported-rootfs-rust-git-2.0.0.tar`
