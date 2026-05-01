# Buildroot Polyglot Squashfs

Heaviest active runnable Buildroot example.

What it covers:
- `rust` artifact
- `go` artifact
- `java` artifact
- `node` artifact
- `python` artifact
- install mappings for each
- simple OS tweaks through stage files, env, and a service unit
- one Buildroot image definition with a real non-`tar` squashfs output
- one checkpoint

Useful commands:
```bash
CARGO_TARGET_DIR=.gaia/examples/buildroot-polyglot-squashfs/local-target cargo run -p gaia -- validate examples/buildroot-polyglot-squashfs/build.toml
CARGO_TARGET_DIR=.gaia/examples/buildroot-polyglot-squashfs/local-target cargo run -p gaia -- plan examples/buildroot-polyglot-squashfs/build.toml
CARGO_TARGET_DIR=.gaia/examples/buildroot-polyglot-squashfs/local-target cargo run -p gaia -- run examples/buildroot-polyglot-squashfs/build.toml
```

Why this exists:
- Buildroot source declared through Gaia
- custom squashfs defconfig path
- multiple artifacts and install destinations
- staged hostname, motd, env, and service content
- final non-`tar` image contract under `.gaia/examples/buildroot-polyglot-squashfs/out`
