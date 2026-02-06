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

Documentation:
- `docs/README.md`

## CLI

```bash
# Show resolved task plan
cargo run -- plan configs/builds/HeliOS-cm5.toml

# Run build
cargo run -- run configs/builds/HeliOS-cm5.toml --max-parallel 0

# Dry run (no task bodies executed)
cargo run -- run configs/builds/HeliOS-cm5.toml --dry-run

# Print resolved config
cargo run -- resolve configs/builds/HeliOS-cm5.toml

# TUI
cargo run -- tui
```

## Repo Layout

- `crates/gaia-image-builder/` main Gaia crate
- `crates/gaia-image-builder-macros/` proc-macro crate
- `configs/` TOML build definitions/modules/targets/workspace/env
- `assets/` non-TOML build/runtime assets (Buildroot tree, services, overlays, templates, SDK cache)

This repository is a Cargo workspace with both crates registered in
`Cargo.toml`.

Build artifact behavior (collect output location, archive, shrink, report) is
configured under `[buildroot]` in module/build files.
