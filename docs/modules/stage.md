# Stage Module

Source: `crates/gaia-image-builder/src/modules/stage.rs`

`stage.render` materializes a rootfs overlay under:

- `<workspace.build_dir>/stage/<build-name>/rootfs`

## Stage files

`[[stage.files]]` entries support:

- `src` + `dst` (copy file/dir/symlink)
- `content` + `dst` (inline file content)
- optional `mode`

`dst` is an absolute image path (for example `/etc/hostname`).

## Environment files

`[stage.env]` supports:

- `sets`: named env maps for service reuse
- `files`: explicit env files written to image paths

## Services and systemd units

`[stage.services.units.<name>]` supports:

- `src` (unit file path) or `vendor = true` (use distro vendor unit)
- `unit` override name
- `targets` for enable symlinks (`*.wants`)
- `env_set` and inline `env`
- `assets` copied into image paths

Gaia writes unit files into `/etc/systemd/system`, plus enable symlinks.

## Safety guards

- rejects invalid unit names / target names
- rejects parent-dir traversal in sensitive paths
- enforces workspace-root boundary for certain asset resolutions

## Manifest output

`stage.render` writes:

- `<out>/<build>/gaia/modules/stage/manifest.json`

This captures rendered files, units, assets, env files, and symlink intents.

## HeliOS examples

HeliOS-style example layout:

- system files:
  - `configs/modules/stage/system.toml`
- env sets:
  - `configs/env/runtime/helios.toml`
- service packs:
  - `configs/modules/stage/services/*.toml`

Patterns worth copying:

- split service configuration by operational domain (runtime/gadget/provisioning/ssh/etc.)
- use `env_set` + `env_file` to keep unit files generic and environment-specific values in TOML
- bundle scripts/config assets directly in service entries to keep runtime dependencies explicit
