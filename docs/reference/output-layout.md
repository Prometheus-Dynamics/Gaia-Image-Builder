# Output Layout Reference

## Workspace roots

From `[workspace]`:

- `root_dir`
- `build_dir`
- `out_dir`

## Runtime directories used by modules

Derived in `crates/gaia-image-builder/src/modules/util.rs`.

- stage root overlay:
  - `<build_dir>/stage/<build-name>/rootfs`
- artifact registry:
  - `<build_dir>/artifacts`
- checkpoint store:
  - `<build_dir>/checkpoints`
  - `<build_dir>/checkpoints/index.json`
  - `<build_dir>/checkpoints/upload-queue.json`
  - `<build_dir>/checkpoints/points/<checkpoint-id>/<fingerprint>/manifest.json`
  - `<build_dir>/checkpoints/points/<checkpoint-id>/<fingerprint>/payload/`
  - `<build_dir>/checkpoints/points/<checkpoint-id>/<fingerprint>/payload.tar`
- starting-point extraction cache (when `buildroot.starting_point.rootfs_tar` or `image` is used):
  - `<build_dir>/starting-point/<build-name>/extract`
- gaia run directory:
  - `<out_dir>/<build-name>/gaia`
- module manifests:
  - `<out_dir>/<build-name>/gaia/modules/<module-path>/manifest.json`

## Buildroot outputs

- raw Buildroot output tree:
  - `<buildroot.br_output_dir>` (default under `<build_dir>`)
- collected images:
  - `<buildroot.collect_out_dir>` or default `<out_dir>/<build>/gaia/images`
- optional archive:
  - alongside collected images parent
- optional report:
  - `<out_dir>/<build>/gaia/image-report.json`

## Useful manifest files

- `resolved.toml`
- `manifest.json` (collect manifest)
- `image-report.json` (if enabled)
- per-module manifests under `gaia/modules/...`
