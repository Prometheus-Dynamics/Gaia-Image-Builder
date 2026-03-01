# PhotonVision HeliOS Raze Minimal (Gaia-only)

This example replaces the old image-modifier/chroot flow with a single Gaia Buildroot pipeline.

It is intentionally minimal:
- Buildroot produces the base image.
- Gaia stages PhotonVision runtime and helios-raze camera seeding assets.
- A base checkpoint is captured/restored at `buildroot.build`.

## What is included

- PhotonVision systemd service (`photonvision.service`)
- helios-raze camera DB seed service (`helios-seed-photonvision-camera.service`)
- helios `hardwareConfig.json`
- helios-raze `ov9782-overlay.dts` staged into `/boot/overlays` for sensor-specific customization
- Buildroot package set focused on PhotonVision runtime + libcamera camera enumeration
- includes `libdrm`/`mesa3d` runtime deps expected by `photon-libcamera-gl-driver` JNI usage

## What is intentionally not included

- Legacy RPiOS image-modifier scripts/chroot path
- Gadget/networkd/dnsmasq provisioning stack
- fan/LED/bootloader update service packs
- automatic OV9782 dtbo compilation (provide your own dtbo pipeline if enabling ov9782 overlay in `config.txt`)

## Build inputs and runtime toggles

This example exposes input toggles in `gaia/configs/program.toml`:

- `pv_jar_source = release|local|repo` (default: `release`)
- `pv_driver_source = off|release|local|repo` (default: `off`)

Set them from CLI with `--set key=value`:

```bash
# default path: PhotonVision jar from GitHub release URL, no driver artifact
cargo run -- run examples/photonvision-helios-raze-minimal/gaia/build.toml --set pv_jar_source=release

# local PhotonVision jar
export PHOTONVISION_JAR_PATH=/abs/path/to/photonvision-linuxarm64.jar
cargo run -- run examples/photonvision-helios-raze-minimal/gaia/build.toml --set pv_jar_source=local

# repo build path (driver runs first, then PhotonVision jar build)
export PHOTONVISION_REPO_DIR=/abs/path/to/photonvision
export PHOTON_LIBCAMERA_DRIVER_JAR_URL=https://.../photon-libcamera-gl-driver.jar
cargo run -- run examples/photonvision-helios-raze-minimal/gaia/build.toml \
  --set pv_jar_source=repo \
  --set pv_driver_source=release
```

`program.custom.artifacts[].after_artifacts = ["photon-libcamera-driver-jar?"]` enforces ordering when the driver artifact is enabled.

## Run

From repo root:

```bash
cargo run -- resolve examples/photonvision-helios-raze-minimal/gaia/build.toml
cargo run -- plan examples/photonvision-helios-raze-minimal/gaia/build.toml --set pv_jar_source=release
cargo run -- checkpoints status examples/photonvision-helios-raze-minimal/gaia/build.toml
cargo run -- run examples/photonvision-helios-raze-minimal/gaia/build.toml --max-parallel 0 --set pv_jar_source=release
```

Second run with unchanged Buildroot inputs should restore the `base-os` checkpoint and skip compile-heavy Buildroot work.

## Optional remote checkpoint uploads (env-backed creds)

`configs/checkpoints.toml` includes a commented S3 backend block using env vars:
- `GAIA_CP_S3_BUCKET`
- `GAIA_CP_S3_ENDPOINT`
- `GAIA_CP_AWS_KEY`
- `GAIA_CP_AWS_SECRET`

Uncomment the backend + point backend reference to enable upload/download.
