# Base OS Starting-Point Example

Small, fast examples for starting-point and checkpoint behavior without running
a full Buildroot compile.

## Example builds

- `build-dir.toml`: starting point from `inputs/seed-rootfs` (`rootfs_dir`).
- `build-tar.toml`: starting point from `inputs/base-rootfs.tar` (`rootfs_tar`).
- `build-checkpointed.toml`: starting-point flow with checkpoints configured.

## Run

From repo root:

```bash
# rootfs_dir example
cargo run -- run examples/base-os-starting-point/build-dir.toml

# create tar input from seed rootfs, then run tar example
bash examples/base-os-starting-point/scripts/make-base-rootfs-tar.sh
cargo run -- run examples/base-os-starting-point/build-tar.toml

# checkpoint-enabled starting-point example
cargo run -- run examples/base-os-starting-point/build-checkpointed.toml
cargo run -- checkpoints status examples/base-os-starting-point/build-checkpointed.toml
```

Outputs are under `examples/base-os-starting-point/out/`.
