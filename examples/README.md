# Examples

This directory has two kinds of content:

- runnable examples you can validate, plan, run, and inspect
- templates you copy when starting a new Gaia build

Generated example build state, Cargo targets, and outputs live under `.gaia/examples/<example>/`.
Clean all runnable example artifacts with `rm -rf .gaia/examples`.

## Runnable Examples

### Imported Base Image Flows

- [imported-rootfs-minimal/README.md](imported-rootfs-minimal/README.md)
  Smallest imported-rootfs example.
  Use it when you want the fastest proof that Gaia can turn an existing rootfs directory into a real output tar.
  Output: `rootfs.tar`

- [imported-raw-image-mutate/README.md](imported-raw-image-mutate/README.md)
  Privileged imported-raw-image example.
  Use it when you need to start from an existing `.img`, mutate it, install packages, and apply Gaia overlay content.
  Output: mutated `.img`

- [imported-rootfs-rust-git/README.md](imported-rootfs-rust-git/README.md)
  Imported-rootfs plus pulled Rust repo example.
  Use it when you want the smallest “pull repo, build project, install into image” path.
  Output: `rootfs.tar`

- [imported-rootfs-polyglot-git/README.md](imported-rootfs-polyglot-git/README.md)
  Imported-rootfs plus polyglot repo build example.
  Use it when you want the richer “pull repos, build multiple languages, install into image” path.
  Output: `rootfs.tar`

- [imported-rootfs-cross-aarch64/README.md](imported-rootfs-cross-aarch64/README.md)
  Imported-rootfs plus cross-target repo build example.
  Use it when you need a pulled-project path that proves non-host-target Rust and Go artifacts are actually installed as `aarch64` binaries.
  Output: `rootfs.tar`

### Buildroot Flows

- [buildroot-rust-minimal/README.md](buildroot-rust-minimal/README.md)
  Fastest Buildroot example.
  Use it for quick iteration on the core Buildroot flow with one Rust binary and one OS change.
  Output: `rootfs.tar`

- [buildroot-rust-squashfs/README.md](buildroot-rust-squashfs/README.md)
  Non-`tar` root filesystem example.
  Use it when you want to prove Gaia updates final squashfs outputs after overlay.
  Output: `rootfs.squashfs`

- [buildroot-rust-sdcard/README.md](buildroot-rust-sdcard/README.md)
  Generic disk-image example.
  Use it when you want to prove Gaia can emit a final `sdcard.img` and promote it as the primary deliverable.
  Output: `sdcard.img`

- [buildroot-rust-aarch64/README.md](buildroot-rust-aarch64/README.md)
  Cross-target Rust Buildroot example.
  Use it when you need a real `aarch64` artifact installed into a real `aarch64` Buildroot image.
  Output: `rootfs.tar`

- [buildroot-raspberrypi4-go/README.md](buildroot-raspberrypi4-go/README.md)
  Board-specific Raspberry Pi 4 example.
  Use it when you want the board-image proof path: real Pi-oriented Buildroot config, real `sdcard.img`, real target binary.
  Output: `sdcard.img`

- [buildroot-polyglot-squashfs/README.md](buildroot-polyglot-squashfs/README.md)
  Heaviest active Buildroot example.
  Use it when you want a small distro-style config with multiple artifacts, runtime content, checkpoints, and a real non-`tar` output contract.
  Output: `rootfs.squashfs`

## Templates

- [templates/full/README.md](templates/full/README.md)
  Full commented template split across multiple files.

- [templates/minimal-buildroot/build.toml](templates/minimal-buildroot/build.toml)
  Smallest commented Buildroot template.

- [templates/minimal-starting-point/build.toml](templates/minimal-starting-point/build.toml)
  Smallest commented imported-base-image template.

Templates are meant to be copied and edited, not run unchanged.
