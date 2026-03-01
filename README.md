# Gaia Image Builder

```bash
cargo install --path crates/gaia-image-builder --bin gaia --force
```

Gaia composes and runs image builds from TOML configs using three generic
pipelines:
- `buildroot` (base OS build)
- `program` (program build + install mapping)
- `stage` (services/files/env staging)

Current build entrypoint:
- `configs/builds/HeliOS-cm5.toml`

Starting-point example entrypoints:
- `examples/base-os-starting-point/build-dir.toml`
- `examples/base-os-starting-point/build-tar.toml`
- `examples/base-os-starting-point/build-checkpointed.toml`

PhotonVision minimal Gaia-only example:
- `examples/photonvision-helios-raze-minimal/gaia/build.toml`

Documentation:
- `docs/README.md`

## CLI

```bash
# Initialize a minimal scaffold in ./gaia
cargo run -- init

# Initialize in a custom directory
cargo run -- init my-image

# Show resolved task plan
cargo run -- plan configs/builds/HeliOS-cm5.toml

# Show a mode-specific plan using build inputs
cargo run -- plan examples/photonvision-helios-raze-minimal/gaia/build.toml --set pv_jar_source=repo --set pv_driver_source=release

# Run build
cargo run -- run configs/builds/HeliOS-cm5.toml --max-parallel 0

# Dry run (no task bodies executed)
cargo run -- run configs/builds/HeliOS-cm5.toml --dry-run

# Print resolved config
cargo run -- resolve configs/builds/HeliOS-cm5.toml

# Checkpoint decision preview
cargo run -- checkpoints status configs/builds/HeliOS-cm5.toml

# Retry pending checkpoint uploads
cargo run -- checkpoints retry configs/builds/HeliOS-cm5.toml

# List checkpoint fingerprints (local + remote)
cargo run -- checkpoints list configs/builds/HeliOS-cm5.toml --remote

# TUI
cargo run -- tui
```

Environment variables from a `.env` file are loaded at startup (`dotenv`), so
checkpoint backend secrets can be supplied via env-backed config fields.

Buildroot can also be bypassed with `[buildroot.starting_point]` using a prebuilt
`rootfs_dir`, `rootfs_tar`, or disk `image`, while keeping the same Gaia flow for
`program.*`, `stage`, and `buildroot.collect`.

## Repo Layout

- `crates/gaia-image-builder/` main Gaia crate
- `crates/gaia-image-builder-macros/` proc-macro crate
- `configs/` TOML build definitions/modules/targets/workspace/env
- `assets/` non-TOML build/runtime assets (Buildroot tree, services, overlays, templates, SDK cache)

This repository is a Cargo workspace with both crates registered in
`Cargo.toml`.

Build artifact behavior (collect output location, archive, shrink, report) is
configured under `[buildroot]` in module/build files.
