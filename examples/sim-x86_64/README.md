# sim-x86_64

A target anyone can build and run without a board. An x86_64 Buildroot base
(glibc, systemd, no kernel or bootloader) is split into layers, and an app
built outside Buildroot is the top layer. Gaia writes all of them as an OCI
image that podman or docker boots with systemd as init.

```sh
gaia run examples/sim-x86_64/build.toml
tar -C .gaia/examples/sim-x86_64/out/images/sim.oci -cf sim.tar .
podman load -i sim.tar
podman run --rm -it --systemd=always sim-x86_64:dev
```

## Layers

| Layer | From | Holds |
| --- | --- | --- |
| `base` | Buildroot's `rootfs.tar` (`[image.layers]`, the default) | glibc, systemd, BusyBox, dbus |
| `platform` | the stage files `sim-board` and `sim-fstab` | `/etc/sim/board`, and an `/etc/fstab` without the `/dev/root` remount a container refuses (it overrides the skeleton's) |
| `app` | the assembly tree `app` | `/usr/bin/sim-app` and its systemd unit, enabled |

## The app-layer fast path

The app is not installed into the image: the assembly copies it from the
artifact's output into its tree. The image build does not wait for the
artifact, the assembly does, so an app-only change rebuilds the app and the
assembly and reuses Buildroot. The app links against the image's own glibc:
its `[[toolchains]]` entry points at the Buildroot output's `host/` dir, and
`after_image_prepare = true` builds it once that toolchain exists.

Measured on 2026-11-01 (Fedora 44, 16 cores):

| Run | Time |
| --- | --- |
| Cold (Buildroot toolchain, glibc, systemd) | 33 min for `image:prepare`, then 22 s |
| App-only change | 1.25 s: artifact 0.8 s, assembly 0.16 s |

The OCI image is 17.2 MiB (gzip). On the app-only run, the base and platform
blobs were reused and only the app layer was written again.
