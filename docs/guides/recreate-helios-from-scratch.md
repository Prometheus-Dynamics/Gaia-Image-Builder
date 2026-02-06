# Recreate HeliOS From Scratch

This guide is the complete blueprint for rebuilding a HeliOS-style Gaia setup without copying the original files directly.

Goal: by the end, you can reconstruct the same architecture from an empty repo with only this document and Gaia.

No external HeliOS checkout is required for this guide.

## 1) Understand the final composition

HeliOS is composed as layers:

1. build entrypoint (`configs/builds/HeliOS-cm5.toml`)
2. workspace defaults (`configs/workspace/base.toml`)
3. distro composition (`configs/distros/helios/base.toml`)
4. platform/board overlays (`configs/platforms/pi/*.toml`)
5. module fragments (`configs/modules/**`)
6. runtime env sets (`configs/env/runtime/helios.toml`)
7. assets staged into the image (`assets/**`)

The top build file is tiny. Most behavior comes from imports.

## 2) Create the repo skeleton

Start with this structure:

```bash
mkdir -p \
  configs/{builds,workspace,distros/helios,platforms/pi,env/runtime} \
  configs/modules/{buildroot,program,stage/services} \
  assets/{buildroot,services,overlays,libcamera,pipelines,lighting}
```

You can add files incrementally and keep validating with `gaia resolve` and `gaia plan`.

## 3) Build entrypoint

Create `configs/builds/HeliOS-cm5.toml`:

```toml
imports = [
  "../workspace/base.toml",
  "../distros/helios/base.toml",
  "../platforms/pi/cm5.toml",
]

[build]
version = "v2026.0.0"

[buildroot]
collect_out_dir = "output/HeliOS-cm5/{version}/images"
archive_mode = "image"
archive_name = "HeliOS-cm5-{version}-sdcard"
```

That mirrors the canonical HeliOS build shape.

## 4) Workspace defaults

Create `configs/workspace/base.toml`:

```toml
[workspace]
root_dir = "."
build_dir = "build"
out_dir = "output"
clean = "none"

[workspace.paths]
# optional aliases
```

`clean` controls deletion behavior during `core.init`.

## 5) Platform layer (Raspberry Pi CM5)

Create `configs/platforms/pi/base.toml` with Pi-wide settings:

- `buildroot.rpi.arch = "aarch64"`
- `buildroot.rpi.config_file = "assets/buildroot/config.txt"`
- `buildroot.rpi.cmdline_file = "assets/buildroot/cmdline.txt"`
- `program.profiles.aarch64-linux.target = "aarch64-unknown-linux-gnu"`
- `program.profiles.aarch64-linux.container_image = "helios-cross"`

Create `configs/platforms/pi/cm5.toml`:

```toml
imports = ["./base.toml"]

[buildroot]
expand_size_mb = 2300

[buildroot.rpi]
board = "cm5"
defconfig = "raspberrypicm5io_defconfig"
overlay = "assets/overlays/cm5"

[program.profiles.aarch64-linux.env]
RUSTFLAGS = "-C target-cpu=cortex-a76 --cfg feature=\"ffmpeg_8_0\""
```

## 6) Distro composition layer

Create `configs/distros/helios/base.toml` to assemble module fragments.

HeliOS pattern:

- Buildroot base + package overlay
- Program base + rust/java/custom + install
- Stage base + system + service packs
- Runtime env imported into `stage.env`

Use import composition exactly so each concern remains modular.

## 7) Buildroot module fragments

Create:

- `configs/modules/buildroot/base.toml`
- `configs/modules/buildroot/helios_packages.toml`
- `configs/modules/buildroot/package_versions.toml`

### 7.1 Base behavior

In `base.toml`, include at least:

- Buildroot repo/version
- source/output dirs (`src_dir`, `br_output_dir`)
- output/archive behavior
- performance/cache settings
- step labels (`steps.fetch/configure/build/collect`)

### 7.2 HeliOS package set

In `helios_packages.toml`, reproduce these patterns:

- `external = ["assets/buildroot"]` to inject custom package/board trees
- package toggles for `systemd`, `openssh`, `dnsmasq`, `ffmpeg`, `libcamera`, toolchains
- symbol overrides for rootfs label/password/users table and FFmpeg/libcamera features

### 7.3 Board assets needed by external tree

Create and wire:

- `assets/buildroot/boards/raspberrypicm5io/genimage.cfg.in`
- `assets/buildroot/boards/raspberrypicm5io/post-image.sh`
- `assets/buildroot/boards/raspberrypicm5io/post-build.sh`
- `assets/buildroot/boards/raspberrypicm5io/users.table`
- `assets/buildroot/cmdline.txt`
- `assets/buildroot/config.txt`

These are central to the CM5 boot image generation flow.

## 8) Program module fragments

Create:

- `configs/modules/program/base.toml`
- `configs/modules/program/rust.toml`
- `configs/modules/program/custom.toml`
- `configs/modules/program/java.toml`
- `configs/modules/program/lint.toml`
- `configs/modules/program/install.toml`

### 8.1 Base program policy

`base.toml` should define:

- `default_profile = "aarch64-linux"`
- reusable checks (`cargo-fmt`, `cargo-clippy`, frontend lint)
- optional host profile for non-cross frontend work

### 8.2 Rust artifacts

`rust.toml` defines HeliOS runtime binaries and plugins as artifacts:

- `helios-api`, `helios-engine`, `helios-updater`, `helios-usb-recoveryd`
- peripherals/control daemons
- plugin `.so` artifacts (`cdylib`)

### 8.3 Frontend artifact

`custom.toml` builds and stages a Bun frontend bundle as a directory artifact.

### 8.4 Install mapping

`install.toml` maps each artifact to its in-image destination, for example:

- `/usr/bin/helios-api`
- `/usr/lib/helios/plugins/daedalus/*.so`
- `/opt/helios/frontend`

## 9) Stage module fragments

Create:

- `configs/modules/stage/base.toml`
- `configs/modules/stage/system.toml`
- `configs/modules/stage/services/*.toml`
- `configs/env/runtime/helios.toml`

### 9.1 System files (`system.toml`)

Add:

- identity files (`/etc/hostname`, `/etc/os-release`, `/etc/default/helios-identity`)
- locale/timezone
- network defaults (`eth0`, `usb0`)
- camera + template assets
- baseline env files under `/etc/helios/*.env`

### 9.2 Runtime env sets (`env/runtime/helios.toml`)

Define named sets:

- `helios-api`
- `helios-engine`
- `helios-usb-recoveryd`
- `helios-peripherals`
- `helios-boot-buttond`

Then reference them from services via `env_set` and `env_file`.

### 9.3 Service packs

Split service concerns into files (as HeliOS does):

- provisioning
- gadget
- networkd
- ssh
- login banner
- runtime core
- usb power

This keeps large service fleets maintainable.

## 10) Recreate key service patterns

Use these HeliOS patterns when writing unit files:

### 10.1 Socket-backed daemon pair

- `helios-engine.socket` listens on `/run/helios/engine.sock`
- `helios-engine.service` includes `Requires=helios-engine.socket` and `Also=helios-engine.socket`

### 10.2 Stateful service with data mount dependency

`helios-api.service` and `helios-engine.service` use:

- `RequiresMountsFor=/var/lib/helios`
- `ExecStartPre` checks for mountpoint
- `StateDirectory=` and `LogsDirectory=`

### 10.3 First-boot provisioning chain

Use a target + oneshot service + mount unit:

- `helios-provision.target`
- `helios-provision.service` (partition/mkfs logic)
- `var-lib-helios.mount` from `LABEL=DATA`

### 10.4 Gadget stack

- `helios-usb-gadget.service` runs setup script from staged asset
- dnsmasq and watchdog units are separate
- diagnostics hooks added by service drop-ins

## 11) Validate each layer as you build

Run these repeatedly while adding files:

```bash
cargo run -- resolve configs/builds/HeliOS-cm5.toml
cargo run -- plan configs/builds/HeliOS-cm5.toml
cargo run -- run configs/builds/HeliOS-cm5.toml --dry-run
```

Expected major tasks in a full HeliOS-style pipeline:

1. `core.init`
2. `program.*` build tasks (if enabled)
3. `program.install.stage`
4. `stage.render`
5. `buildroot.rpi.validate`
6. `buildroot.rpi.prepare`
7. `buildroot.fetch`
8. `buildroot.configure`
9. `buildroot.build`
10. `core.barrier.stage`
11. `buildroot.collect`

## 12) Build and inspect outputs

Run a real build:

```bash
cargo run -- run configs/builds/HeliOS-cm5.toml --max-parallel 0
```

Inspect:

- `output/HeliOS-cm5/gaia/resolved.toml`
- `output/HeliOS-cm5/gaia/manifest.json`
- `output/HeliOS-cm5/gaia/image-report.json` (if enabled)
- `build/stage/HeliOS-cm5/rootfs`

## 13) Reconstruction checklist

If all items are present, you have effectively recreated HeliOS architecture:

- layered import structure (workspace/distro/platform/modules)
- external buildroot tree and board assets
- cross + host program artifact pipelines
- stage system + env + services split by concern
- provisioning + data mount + socket services + gadget stack
- deterministic output collection/archive/report flow
