# Buildroot Raspberry Pi 4 Go

Board-oriented Raspberry Pi 4 Buildroot example that proves:

- Gaia emits a real `sdcard.img` board image.
- Gaia cross-builds a Go app for the target and places it into the final image.

This example uses:

- a repo-owned Raspberry Pi 4 Buildroot defconfig tuned for smoke runs
- global Docker execution for Buildroot/source work
- a per-artifact Docker override for the Go app build

Build the helper image once:

```bash
./scripts/build-buildroot-smoke-image.sh
```

Run the example:

```bash
CARGO_TARGET_DIR=.gaia/examples/buildroot-raspberrypi4-go/local-target cargo run -p gaia -- run examples/buildroot-raspberrypi4-go/build.toml
```

Primary output:

- `.gaia/examples/buildroot-raspberrypi4-go/out/images/buildroot-raspberrypi4-go-2.0.0.img`

Expected payload proof:

- `/usr/bin/pi-dummy`
- `/etc/motd`
