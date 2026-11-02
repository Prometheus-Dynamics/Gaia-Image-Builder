# TODO

Open work, checked against the code on 2026-11-01. Every open item says what it
is, why, when it is done and what blocks it. Finished items are in "Done" with
their commits.

Board work gets tested on boards before anything ships, unless the user says
otherwise. "Written" below means built and checked on the host, not booted.

## Boards: test what is written

- **Boot the board examples**
  - What: boot each image in `examples/boards/` on its board: ROCK 5C, ROCK 4D, Luckfox Pico Pro, BPI-CanMV-K230D-Zero, and the CM4 and Pi 5 cast receivers. Read the RK3576 maskrom USB product id with `lsusb` on the ROCK 4D.
  - Why: all of them build and pass `image check` on the host, but none has booted.
  - Done when: each boots from its image, and `examples/boards/README.md` says so.
  - Blocked by: each of those boards on a bench, with a serial console.
- **Luckfox NAND slots on a board**
  - What: the checks in `examples/boards/README.md#nand-slots`: `mtd list` names `spi-nand0`; a trial of each slot, killed in Linux, returns to the committed slot; `/data` formats and mounts on first boot; `/dev/watchdog` appears; an uncommitted trial resets at about 600 s plus the watchdog timeout, and a commit stands the watchdog down; `/proc/cpuinfo` has a non-zero `Serial`; `atlas ls` finds the board.
  - Why: the slot contract (942746b), the watchdog, `/data` and slot bundle (720c211) and Atlas's device package (17c20df) were written from the SDK's source. The `spi-nand0` name is the riskiest assumption, and if it is wrong a trial cannot fall back by itself. The fix from `rk-nand` to `spi-nand0` in `mtdparts` is also unproven on a board.
  - Done when: every check passes on a Luckfox Pico Pro.
  - Blocked by: a Luckfox Pico Pro on a bench, with a serial console.
- **UBI image on real SPI-NAND**
  - What: attach the image on a board, with no VID offset given.
  - Why: the kernel attached it on nandsim (44fc94b, kernel 7.2), but nandsim has 512-byte sub-pages and was given offset 2048. That a board needs no offset comes from reading the vendor driver.
  - Done when: a Luckfox Pico Pro attaches `ubi.img` at boot with its three volumes.
  - Blocked by: a Luckfox Pico Pro on a bench. Tested together with the slots above.
- **CM5 on-device checks**
  - What: confirm that the `ov9782` and `raze-fan` boot overlays load, once Atlas has fixed their install path (the 2026-10-23 PV camera incident). If Atlas adopts EROFS 1 MiB pclusters (LZMA level 6, 47.2 to 44.2 MB), measure boot time and first-use reads.
  - Why: both change the boot path and cannot be checked off-device.
  - Done when: the overlays load on a CM5, and the pcluster numbers are measured if Atlas adopts them.
  - Blocked by: a Raze slot from the HeliOS coordinator (Raze #1 is a CM5, on the bench behind the SystemCore).

## Boards: emulated

- **Raze OS in QEMU**
  - What: boot a Raze dynamic-slots image in `qemu-system-aarch64 -M virt` on the build host, so HeliOS's whole stack runs without a board: systemd, Orion, lemnosd and Styx in their sim modes, the slots writer's stage, apply, trial and rollback. The user un-cut this on 2026-11-02 (through HeliOS) while the SystemCore and Raze #1 are down. `scripts/qemu-rpi-slots.sh` does the Pi firmware's part and the network (a rootless namespace with DHCP at 10.77.0.50 by default). On 2026-11-02 HeliOS's board image ef4f1f2f, booted with a Raze kernel carrying the qemu fragment, reached `running` with no failed unit but the USB gadget; SSH, helios-api, the board agent and Orion answered on 10.77.0.50; a `reboot "0 tryboot"` booted tryboot.txt and slot-init started the slot as a trial.
  - Why: the boards are away for hours; HeliOS's smoke test (update refusal, stored-graph refresh, Orion persistence, the UI) needs a running OS.
  - Done when: a Raze image built with the qemu fragment by Buildroot (its own kernel, so no `--kernel`) boots the same way, with Styx and lemnosd in sim mode, and a staged slot confirms or rolls back.
  - Blocked by: Atlas, for `BR2_RAZE_KERNEL_QEMU` (the fragment appended after `raze.config` and `raze-trim.config`); HeliOS, for its qemu layer (`input.machine = "qemu"`, the sim settings). A kernel built outside Buildroot must use the board's exact `.config` and Bootlin toolchain: with Fedora's GCC 16 or a re-derived config the board's modules refuse to load (MODVERSIONS CRCs on the block layer).

## Boards: images and support

- **Remote install on the Luckfox**
  - What: a way for Atlas to start `atlas-ubi nand-slot install` on the board. The image has no SSH, Orion or USB console, and `board-lite` is read-only.
  - Why: without it, an update can only be started by hand on the serial console.
  - Done when: Atlas installs and commits a slot from another machine.
  - Blocked by: the user's security decision, which Atlas has taken to the user. Atlas then supplies the layer; a USB gadget may also need kernel or device-tree changes here.
- **G8: NAND layouts other than the Luckfox**
  - What: Rockchip's SPI-NAND idblock layout for the L variants, and the K230 NAND image layout. Gaia has no code for either: the Luckfox uses the idblock its vendor SDK's `make.sh` builds.
  - Why: the K230D boards and the Rockchip L variants boot from SPI-NAND.
  - Done when: each layout is built with byte-level tests against the vendor SDK's format, and boots on its board.
  - Blocked by: nothing for the formats (the K230 SDK and rkbin are fetched). The board for the boot.
- **G6: per-board size budget**
  - What: a whole-device budget for disk images (eMMC/SD), so a disk larger than the board's part fails and names the smallest size it needs, and the smallest-flash number for each board.
  - Why: two slots plus data has to fit the cheapest flash. Raw NAND is covered: `flash_size` fails the build when the volumes do not fit, and UBI builds report `minimum_flash_bytes` (1a81221). A disk image only fails when an image is larger than its partition.
  - Done when: a current real HeliOS and PV build pass and an oversized one fails, and the numbers go to the PCB session.
  - Blocked by: nothing. The board examples supply the configs; the check has to run on real HeliOS and PV builds before it can fail a build.
- **Rust on the K230D**
  - What: link Rust artifacts for the K230D through LLD, in the K230 example or by default for the XuanTie toolchain.
  - Why: Rust 1.99 objects for riscv64 do not link with the XuanTie V2.10.1 GNU `ld`, profile or not (`unknown z ISA extension: 'zaamo'`). LLD links them (docs/configuration-sources.md#cpu-profiles).
  - Done when: a Rust artifact builds in the K230D example and its binary links.
  - Blocked by: nothing.
- **CPU profile checks on boards**
  - What: on each Rockchip board, `/proc/cpuinfo` lists `aes` and `sha2`, since the A53 and A55 profiles enable them; on the K230D, RVV binaries run only on CPU1.
  - Why: a missing extension kills a binary with `SIGILL`. Both are assumptions from the chips' documentation, not seen on a board.
  - Done when: checked on each board.
  - Blocked by: the boards.
- **G7: board examples left**
  - What: examples for the RK3566 and RK3562, and a K230 SPI-NAND layout (the Zero only has a TF slot).
  - Why: each chip needs a full config to build from.
  - Done when: each example builds where its sources are present, and is added to `crates/core/app/tests/examples_boards.rs`.
  - Blocked by: nothing.
- **Noisy firmware warnings**
  - What: fewer `asks for firmware` lines from `image check`, for example one summary per image with the list in the report.
  - Why: the ROCK 5C build printed 832 such lines, which bury warnings that matter. The Luckfox prints two, for `cfg80211`'s `regulatory.db`.
  - Done when: a ROCK 5C build prints a short summary, and the whole list is still in its report.
  - Blocked by: nothing.
- **K230D BSP (HVS_U01, HVS_I02)**
  - What: Canaan's Linux-only SDK with Linux on the big C908 (CPU1, the only core with RVV), vendor U-Boot and OpenSBI, and SPI-NAND boot. Size the media memory reservation from Styx's estimate of 20–25 MB.
  - Why: the smart USB camera and the Inferno host.
  - Done when: a board boots slot A, falls back from a bad slot B, and `grep isa /proc/cpuinfo` shows `v`.
  - Blocked by: the boards (PCB).
- **Rockchip BSP: RK3588S/RK3582 (HVS_MH01) and RK3576 (HVS_MM01)**
  - What: Rockchip's vendor 6.1 kernel with rkisp, rkcif, RGA, MPP (rkvenc) and RKNN, but no rkaiq and no librga. Boot chain: mainline U-Boot, TF-A and the rkbin DDR blob. GPU: libmali with Vulkan (Daedalus). eMMC (E) and SPI-NAND (L) variants. The ROCK examples build a mainline kernel (6.19), so the vendor kernel is fetched but not built by any example yet.
  - rkbin licence (checked 2026-10-30 at rkbin 3e288fe): Rockchip grants a non-exclusive licence to use, copy and distribute. It forbids decompiling or reverse engineering, and the copyright notices must stay. Ship rkbin's `LICENSE` in the image's licence directory and in each export; the ROCK examples do not do this yet. DDR blobs for RK3588, RK3576, RK3566 and RK3562 are in `bin/rk35`.
  - Why: the CM5-format camera modules.
  - Done when: each module boots A/B from both storage variants, and Styx and Daedalus see their devices.
  - Blocked by: the boards.
- **Mainline RK3588S track**
  - What: a second kernel option for the RK3588S, with capture working.
  - Why: it is needed once rkisp2 and rkvenc are upstream. The ROCK 5C example already builds mainline, without them.
  - Done when: the module boots mainline with capture working.
  - Blocked by: rkisp2 and rkvenc landing upstream, and a board.
- **Flame BSP (RV1106G2, HVS_F02)**
  - What: an RV1106 vendor BSP (armv7) with SPI-NAND boot and the same support as the other targets: EROFS and verity slots, layers, env sets and size reporting. The Luckfox Pico Pro example is the same chip family and builds from the RV1106 SDK, and its slots are now EROFS with dm-verity opened by `dm-mod.create=` with no initramfs (Atlas 1cd650e reads the new bundle), written and untested on board.
  - Why: Flame.
  - Done when: it boots A/B with EROFS and verity slots.
  - Blocked by: the board for the boot, and a Flame board config built from the Luckfox's.
- **Low-cost tier: RK3566 and RK3562 (HVS_ML01)**
  - What: both chips (the user said "make both"). The vendor 6.1 BSP and boot chain are shared with the other Rockchip modules. RK3562 boards use 256–512 MB DDR3L, and 16-bit DDR3 boot on the RK3562 is unproven.
  - Device tree inputs (from the PCB session; more pin changes may follow): in `HW-Helios-Platform`, the J1 GPIO map is in `projects/HVS_ML01_DEV01/README.md` ("Software / device tree: J1 GPIO map"). The CM5-header SPI0 (J1 GPIO7–11) uses the RK3562 SPI0 **M1** pin group, not M0. The DDR3L DQ swaps are in `projects/HVS_ML01_DEV01/docs/DDR-MATCHING.md` section 18. Read both again when writing the board's DTS, not this copy.
  - Why: the low-cost module tier.
  - Done when: each chip's module boots A/B from SPI-NAND and from eMMC.
  - Blocked by: the boards.

## Release mechanics

- **Changelog, version and tag**
  - What: move `[Unreleased]` to a version, set the workspace version (now `2.0.1-dev`), tag it, and create the GitHub release. The next one is a major version, because config v3 changes the build-file format.
  - Why: HeliOS, PV and Atlas pin a released Gaia.
  - Done when: `cargo package --workspace` and the gate pass at that version, the tag and release exist, and the three projects pin it.
  - Blocked by: the user, who alone decides releases.

## Pinned for later

The user pinned these on 2026-10-30.

- **Registry backend**
  - What: the registry cache pushed to a real registry. The push and pull code passes its round trip against a fake registry in `crates/core/process/src/registry/tests.rs`.
  - Why: CI and boards share built layers.
  - Done when: a push and pull round trip runs against GHCR.
  - Blocked by: the user, who said yes to GHCR through HeliOS on 2026-11-01. Still needed: the account or organization that owns the packages, a token with `write:packages` for the checkpoint's `token`, and the user's yes in Gaia's chat to the first real push.
- **`gaia run` image job on the self-hosted runner**
  - What: the image job of HeliOS's `.github/workflows/gaia-builds.yml` (814e4893). The workflow's validate jobs already pass on GitHub's hosted runners on pushes to HeliOS's `dev`; pushes to `architecture-overhaul`, where HeliOS integrates, do not trigger them, and Gaia's own `Gaia action` workflow builds `imported-rootfs-minimal` on them (last green on 17c20df), so only HeliOS's image job needs the runner.
  - Why: real image builds per change, not just unit tests.
  - Done when: a `workflow_dispatch` run of that workflow goes green with an image. None has run yet.
  - Blocked by: the user, on `Prometheus-Dynamics/HeliOS`, which has no runners or secrets today, following HeliOS's `docs/ci/gaia-builds.md`: register a runner labelled `gaia` (admin rights, about 100 GB free), add the `EIDOS_READ_TOKEN` secret with `gh secret set EIDOS_READ_TOKEN --repo Prometheus-Dynamics/HeliOS`, and set the fork-PR policy.
- **Atlas's signing key**
  - What: signed images checked by Atlas on a board. The user chose (2026-11-02, through HeliOS) a dev/example key made in Atlas for now: minisign, no password, the private key at `~/.config/atlas/dev-signing/minisign.key` outside any repo and its path in `ATLAS_DEV_SIGNING_KEY`; the public key and its owner block in Atlas's `keys/trusted-keys.toml`. The board examples sign with it and build unsigned with a warning without it (`optional = true`). A production key comes later and is the user's.
  - Why: signed updates on boards.
  - Done when: Atlas verifies a Gaia-signed export on a board.
  - Blocked by: a board.

## Backlog

Cut and staying cut: TUI cleanup, deb packaging niceties.

## Done

- Config v3 enums: e411aef.
- `examples/sim-x86_64` and the app-layer fast path: a78b53a.
- G1, bootloaders at fixed disk offsets: 0b09e38.
- G2, U-Boot boot model in `image check`: 6ab2216. It checks a FIT's hashes; a FIT signed with RSA is not verified.
- G3, `uboot-env` transform: 56568df, with the `mkenvimage` and `fw_printenv` test in 1966f26. `copies` for the Luckfox's two env copies: 942746b.
- G4, riscv64 aliases and lp64d loader dirs: 8170082.
- G5, `gaia layer delta --max-window`: 69aecf2.
- G9, armv7 aliases and armhf closure: 3e51c14.
- G8, the UBI writer: 1a81221. The kernel attaching the Luckfox image on nandsim: 44fc94b. Atlas read the real `ubi.img`, so it has the volume layout.
- G10, CPU profile flags in cargo and Buildroot: 1966f26, with `cortex-a72` in a2aeb06. Per-board report, measured with Atlas's crates for every profile: docs/configuration-sources.md#cpu-profiles. HeliOS's Raze build uses `cortex-a76` on `architecture-overhaul` (HeliOS 48492ee5, whose commit message reports the full artifact build passing).
- G7, the gate validates and plans every board example and `examples/sim-x86_64`: `crates/core/app/tests/examples_boards.rs`. The examples that exist: ROCK 5C, ROCK 4D, Luckfox Pico Pro and K230D Zero in 006284a and 3d1f249; the cast receivers in a2aeb06 and db7b92a.
- `${source.<id>.commit_count}`: b47a9e5.
- Luckfox NAND slots, written and untested on board: 942746b, 720c211, 17c20df.
- Signing proof: real minisign and cosign sign and verify image outputs and registry entries in `scripts/ci.sh` and GitHub CI, with throwaway keys. cosign 3 had refused Gaia's `--output-signature`; it now writes an offline Sigstore bundle (`<file>.sigstore.json`).
