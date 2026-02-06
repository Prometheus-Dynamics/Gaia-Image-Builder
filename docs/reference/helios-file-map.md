# HeliOS File Map (Concept -> File)

Use this map when reconstructing HeliOS behavior from scratch.

Paths below are relative to your Gaia project root.

## Build composition

- build entrypoint:
  - `configs/builds/HeliOS-cm5.toml`
- distro composition:
  - `configs/distros/helios/base.toml`
- workspace defaults:
  - `configs/workspace/base.toml`
- platform overlays:
  - `configs/platforms/pi/base.toml`
  - `configs/platforms/pi/cm5.toml`

## Buildroot

- base buildroot options:
  - `configs/modules/buildroot/base.toml`
- package toggles/symbols:
  - `configs/modules/buildroot/helios_packages.toml`
- package version pins:
  - `configs/modules/buildroot/package_versions.toml`
- board boot/genimage logic:
  - `assets/buildroot/boards/raspberrypicm5io/post-image.sh`
  - `assets/buildroot/boards/raspberrypicm5io/genimage.cfg.in`
  - `assets/buildroot/boards/raspberrypicm5io/users.table`
- pi boot files used by `buildroot.rpi`:
  - `assets/buildroot/config.txt`
  - `assets/buildroot/cmdline.txt`

## Program pipeline

- shared checks/profiles:
  - `configs/modules/program/base.toml`
- rust artifacts:
  - `configs/modules/program/rust.toml`
- frontend custom artifact:
  - `configs/modules/program/custom.toml`
- install mapping into image:
  - `configs/modules/program/install.toml`
- lint stage toggles:
  - `configs/modules/program/lint.toml`

## Stage core

- stage baseline:
  - `configs/modules/stage/base.toml`
- system files/network/env defaults:
  - `configs/modules/stage/system.toml`
- runtime env sets:
  - `configs/env/runtime/helios.toml`

## Stage services

- runtime core services:
  - `configs/modules/stage/services/runtime_core.toml`
- provisioning services:
  - `configs/modules/stage/services/provisioning.toml`
- gadget services:
  - `configs/modules/stage/services/gadget.toml`
- networkd vendor units:
  - `configs/modules/stage/services/networkd.toml`
- ssh vendor/dropbear choice:
  - `configs/modules/stage/services/ssh.toml`
- login banner overrides:
  - `configs/modules/stage/services/login_banner.toml`
- usb power service:
  - `configs/modules/stage/services/usb_power.toml`

## Key unit assets

- API:
  - `assets/services/helios-api/helios-api.service`
  - `assets/services/helios-api/helios-api-startup-sanitize.sh`
- Engine:
  - `assets/services/helios-engine/helios-engine.service`
  - `assets/services/helios-engine/helios-engine.socket`
- Updater:
  - `assets/services/helios-updater/helios-updater.service`
  - `assets/services/helios-updater/helios-updater.socket`
- USB recovery:
  - `assets/services/helios-usb-recoveryd/helios-usb-recoveryd.service`
- Provisioning/mount:
  - `assets/services/expand-rootfs/helios-provision.service`
  - `assets/services/expand-rootfs/provisions.toml`
  - `assets/services/storage/helios-data.mount`
- USB gadget:
  - `assets/services/gadget/etc/systemd/system/helios-usb-gadget.service`
  - `assets/services/gadget/usr/local/bin/usb-gadget-setup.sh`

## Runtime outputs to inspect

After a build, inspect:

- `output/HeliOS-cm5/gaia/resolved.toml`
- `output/HeliOS-cm5/gaia/manifest.json`
- `output/HeliOS-cm5/gaia/image-report.json` (if enabled)
- `build/stage/HeliOS-cm5/rootfs`
