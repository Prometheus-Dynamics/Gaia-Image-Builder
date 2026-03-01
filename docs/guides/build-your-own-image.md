# Build Your Own OS Image

This is the end-to-end path from zero to a custom Gaia image.

If you want the full production-style blueprint, read:

- [Recreate HeliOS From Scratch](recreate-helios-from-scratch.md)
- [HeliOS Recipes](helios-recipes.md)

## 1) Start from the minimal example

Use `examples/helios/` as the base:

- `examples/helios/build.toml`
- `examples/helios/configs/*.toml`
- `examples/helios/assets/etc/*`

Copy it:

```bash
cp -r examples/helios examples/myos
```

## 2) Set workspace scope

Edit `examples/myos/configs/workspace.toml`:

```toml
[workspace]
root_dir = "examples/myos"
```

This keeps builds/artifacts isolated under your example folder.

## 3) Pick board + Buildroot defaults

Edit `examples/myos/configs/buildroot.toml`:

- choose `defconfig`
- optionally set `version`, `collect_out_dir`, archive settings, performance knobs

Minimal usable form:

```toml
[buildroot]
version = "2025.11"
defconfig = "raspberrypicm5io_defconfig"
```

## 4) Define rootfs overlay content

Edit `examples/myos/configs/stage.toml` and assets under `examples/myos/assets/`.

Typical first files:

- `/etc/hostname`
- `/etc/os-release`
- `/etc/motd`

## 5) Validate before full build

```bash
cargo run -- resolve examples/myos/build.toml
cargo run -- plan examples/myos/build.toml
cargo run -- run examples/myos/build.toml --dry-run
```

## 6) Run real build

```bash
cargo run -- run examples/myos/build.toml --max-parallel 0
```

`0` means use all CPU cores.

## 7) Find outputs

By default:

- Gaia runtime/manifests: `<workspace.out_dir>/<build-name>/gaia/`
- staged rootfs overlay: `<workspace.build_dir>/stage/<build-name>/rootfs`
- collected images: `<workspace.out_dir>/<build-name>/gaia/images` (unless `collect_out_dir` overrides)

## 8) Add services and programs

- Add systemd units/assets in `[stage.services]`.
- Add application build pipelines in `[program.rust]`, `[program.java]`, or `[program.custom]`.
- Stage produced binaries with `[program.install]`.

## 9) Add a base OS checkpoint (single flow)

You can cache the base Buildroot compile stage without splitting configs:

```toml
[checkpoints]
enabled = true
default_use_policy = "auto"
default_upload_policy = "off"
trust_mode = "verify"

[[checkpoints.points]]
id = "base-os"
anchor_task = "buildroot.build"
use_policy = "auto"
fingerprint_from = [
  "buildroot.version",
  "buildroot.defconfig",
  "buildroot.packages",
  "buildroot.package_versions",
  "buildroot.symbols",
  "buildroot.external",
]
```

This keeps one normal run flow:

- checkpoint hit: Buildroot compile-heavy steps are restored/skipped
- checkpoint miss: Buildroot recompiles and checkpoint is captured
- `stage`/`program` changes still apply at `buildroot.collect`

## 10) Build from an external base rootfs (no Buildroot compile)

If you already have a base rootfs directory/tar/image, you can skip Buildroot compile tasks:

```toml
[buildroot.starting_point]
enabled = true
rootfs_tar = "inputs/base-rootfs.tar"
apply_stage_overlay = true

[buildroot.starting_point.packages]
enabled = true
manager = "auto"
execute = false
install = ["curl", "htop"]
release_version = "bookworm"
allow_major_upgrade = false
```

Behavior:

- `buildroot.fetch/configure/build` are short-circuited.
- `buildroot.collect` imports external rootfs, applies stage/program content, and writes normal manifests.
- package reconciliation can auto-detect manager/version from `/etc/os-release`, with explicit override support.
- for disk images, use `image = "inputs/base.img"` and optionally `image_partition = "2"`; this mode requires running Gaia as root.

That is the full path to a production-style custom OS image in Gaia.

## Executable examples

For small end-to-end examples, see:

- `examples/base-os-starting-point/`
- `examples/base-os-starting-point/README.md`
- `examples/photonvision-helios-raze-minimal/`
- `examples/photonvision-helios-raze-minimal/README.md`

Run them from repo root:

```bash
# rootfs_dir
cargo run -- run examples/base-os-starting-point/build-dir.toml

# rootfs_tar
bash examples/base-os-starting-point/scripts/make-base-rootfs-tar.sh
cargo run -- run examples/base-os-starting-point/build-tar.toml

# checkpoint + starting-point
cargo run -- run examples/base-os-starting-point/build-checkpointed.toml
cargo run -- checkpoints status examples/base-os-starting-point/build-checkpointed.toml

# CI integration test that executes these examples
cargo test --test base_os_starting_point_integration
```
