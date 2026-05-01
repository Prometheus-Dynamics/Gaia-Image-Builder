# Imported Rootfs Polyglot Git

This example proves Gaia can:

- import a base rootfs tar
- pull multiple git repos
- build Rust, Go, Java, Node, and Python projects from those repos
- install all produced artifacts into the image
- stage runtime file, env, and service content

Prep and run:

```bash
./scripts/build-polyglot-artifacts-image.sh
./examples/imported-rootfs-polyglot-git/scripts/prepare-example.sh
CARGO_TARGET_DIR=.gaia/examples/imported-rootfs-polyglot-git/local-target cargo run -p gaia -- run examples/imported-rootfs-polyglot-git/build.local.toml
./examples/imported-rootfs-polyglot-git/scripts/verify-output.sh
```
