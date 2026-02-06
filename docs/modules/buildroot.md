# Buildroot Modules

Sources:

- `crates/gaia-image-builder/src/modules/buildroot.rs`
- `crates/gaia-image-builder/src/modules/buildroot_rpi.rs`

## `buildroot.rpi`

Optional board preflight/prepare layer.

- `buildroot.rpi.validate`: validates board arch + required files.
- `buildroot.rpi.prepare`: stages board overlays, `config.txt`, `cmdline.txt` into stage root.

Provides `buildroot:target-prepared` for `buildroot.fetch/configure` optional dependency.

## `buildroot`

Main OS pipeline with four tasks.

### 1) `buildroot.fetch`

- clones Buildroot repo if missing
- fetches updates
- checks out `buildroot.version` if set

### 2) `buildroot.configure`

- runs defconfig + olddefconfig
- sets `BR2_ROOTFS_OVERLAY` to Gaia stage root
- applies package toggles/symbol overrides
- configures cache/download/performance settings
- emits `resolved.toml` and configure marker

### 3) `buildroot.build`

- runs package build/finalization (host-finalize)
- records post-image-needed marker

### 4) `buildroot.collect`

- synchronizes stage overlay into Buildroot target
- runs `make target-post-image` when needed
- copies image artifacts to collect directory
- optional ext shrink
- optional archive creation (`img`, `img.xz`, `tar.*`)
- writes manifests and optional image report

## Key buildroot config knobs

`[buildroot]` commonly used fields:

- source/output: `src_dir`, `br_output_dir`
- build tuning: `performance_profile`, `threads`, `top_level_jobs`, `top_level_load`
- output: `collect_out_dir`, `archive_format`, `archive_mode`, `archive_name`
- image size/compression: `expand_size_mb`, `compression`, `shrink_ext`
- package/symbol control: `packages`, `package_versions`, `symbols`
- external trees: `external`

## HeliOS examples

HeliOS-style example layout:

- buildroot module base:
  - `configs/modules/buildroot/base.toml`
- package/symbol overlay:
  - `configs/modules/buildroot/helios_packages.toml`
- CM5 board post-image logic:
  - `assets/buildroot/boards/raspberrypicm5io/post-image.sh`
  - `assets/buildroot/boards/raspberrypicm5io/genimage.cfg.in`

Patterns worth copying:

- keep `buildroot/base.toml` generic and isolate product-specific toggles in a second import
- use `external = [\"assets/buildroot\"]` for package and board override trees
- keep board image assembly logic in a board-scoped `post-image.sh`
