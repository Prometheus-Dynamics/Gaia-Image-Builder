# Module Overview

Built-in modules are registered in `crates/gaia-image-builder/src/modules/mod.rs`.

- `core`: schema guard + workspace init task.
- `program.lint`: reusable pre-build checks.
- `program.rust`: build Rust artifacts (host or containerized).
- `program.java`: build Java artifacts.
- `program.custom`: run arbitrary build commands for custom artifacts.
- `program.install`: stage artifact outputs into rootfs overlay.
- `stage`: render files, env files, and systemd units/assets.
- `buildroot.rpi`: board/boot overlay preparation.
- `buildroot`: Buildroot source/config/build/image collection pipeline.

Read module-specific docs:

- [Core Module](core.md)
- [Program Modules](program.md)
- [Stage Module](stage.md)
- [Buildroot Modules](buildroot.md)
