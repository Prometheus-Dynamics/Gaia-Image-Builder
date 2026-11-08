# TODO

Open work, checked against the code on 2026-11-06. Every open item says what it
is, why, when it is done and what blocks it. Finished items are in "Done" with
their commits.

Board work gets tested on boards before anything ships, unless the user says
otherwise. "Written" below means built and checked on the host, not booted.

## Alpha (Raze)

The next release is an alpha for the Raze (CM5) only (the user, 2026-11-02,
through HeliOS). The other boards follow it, so their items stay below.

- **CM5 on-device checks**
  - What: confirm that the `ov9782` and `raze-fan` boot overlays load, once Atlas has fixed their install path (the 2026-10-23 PV camera incident). If Atlas adopts EROFS 1 MiB pclusters (LZMA level 6, 47.2 to 44.2 MB), measure boot time and first-use reads.
  - Why: both change the boot path and cannot be checked off-device.
  - Done when: the overlays load on a CM5, and the pcluster numbers are measured if Atlas adopts them.
  - Blocked by: a Raze slot from the HeliOS coordinator (Raze #1 is a CM5, on the bench behind the SystemCore).
- **Raze image size budget**
  - What: G6's whole-device budget for the Raze first: two slots and `/data` on the CM5's eMMC, a build that fails when they do not fit and names the smallest eMMC it needs, and the number for the PCB session. Budget against a 16 GB eMMC, the smallest CM5 eMMC the Raze supports (the alpha's CM5s have 32 GB, 29.1 GiB usable), and name it in the failure.
  - Why: the alpha ships on the Raze. Its `.img.xz` grows a few KB a build (51.40 MB at HeliOS ef4f1f2f) and nothing fails it while it still fits.
  - Done when: a current HeliOS Raze build passes and an oversized one fails.
  - Status: the check is in Gaia (66d5f79, `device_size` and `device_reserve` on a disk), run both ways on a real Raze assembly: within budget it reports what is left, over it fails with "needs N MiB ... it fits a G GiB device or larger".
  - Blocked by: Atlas setting `device_size = "14910M"` (a 16 GB eMMC's capacity) and the reserve for the second slot on the slots disk in `devices/raze/gaia/slots.toml`.

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
  - Done when: the board examples and a current PV build pass and an oversized one fails, and the numbers go to the PCB session. The Raze's budget comes first, under Alpha.
  - Blocked by: nothing. The board examples supply the configs; the check has to run on real builds before it can fail a build.
- **Rust on the K230D**
  - What: link Rust artifacts for the K230D through LLD, in the K230 example or by default for the XuanTie toolchain.
  - Why: Rust 1.99 objects for riscv64 do not link with the XuanTie V2.10.1 GNU `ld`, profile or not (`unknown z ISA extension: 'zaamo'`). LLD links them (docs/configuration-sources.md#cpu-profiles).
  - Done when: a Rust artifact builds in the K230D example and its binary links.
  - Status: `linker = "lld"` on a `[[toolchains]]` SDK links glibc binaries through the XuanTie SDK (a Gaia run against V2.10.1: `gnu` fails on `zaamo`, `lld` links). Atlas's `board-lite` already links its static musl build with `rust-lld`. Left: a glibc Rust artifact in the K230D example itself, with the XuanTie SDK as a toolchain.
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

- **K230D BSP (HVS_U01, HVS_I02)**
  - What: Canaan's Linux-only SDK with Linux on the big C908 (CPU1, the only core with RVV), vendor U-Boot and OpenSBI, and SPI-NAND boot. The media memory is sized on the K230D Zero (2026-11-02): a 16 MiB CMA area shared by VGLite, MMZ and the CMA dma-heap, for Styx's rings, Eidos's pyramid and VGLite (10.9 MiB); the BSP boards take the same budget.
  - Why: the smart USB camera and the Inferno host.
  - Done when: a board boots slot A, falls back from a bad slot B, and `grep isa /proc/cpuinfo` shows `v`.
  - Blocked by: the boards (PCB).
- **Rockchip BSP: RK3588S/RK3582 (HVS_MH01) and RK3576 (HVS_MM01)**
  - What: Rockchip's vendor 6.1 kernel with rkisp, rkcif, RGA, MPP (rkvenc) and RKNN, but no rkaiq and no librga. Boot chain: mainline U-Boot, TF-A and the rkbin DDR blob. GPU: libmali with Vulkan (Daedalus). eMMC (E) and SPI-NAND (L) variants. The ROCK examples build a mainline kernel (6.19), so the vendor kernel is fetched but not built by any example yet.
  - rkbin licence (checked 2026-10-30 at rkbin 3e288fe): Rockchip grants a non-exclusive licence to use, copy and distribute. It forbids decompiling or reverse engineering, and the copyright notices must stay. The ROCK 5C, ROCK 4D and Luckfox images ship rkbin's `LICENSE` (d160d7d). DDR blobs for RK3588, RK3576, RK3566 and RK3562 are in `bin/rk35`.
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

## Architecture leap (go, 2026-11-06)

The user's go, through HeliOS. Every shared library comes in two forms: a static one, and a shared one with a Rust core, a small versioned C surface and a static Rust client wrapper, never both in one process. Static fat LTO stays the default; shared mode is for consumers outside the image. Gaia's part:

- **Link modes**
  - What: static by default, shared chosen per image (layers), for every dependency with a surface.
  - Status: `link = "shared"`, `package = "std"` (7e090ec), the per-image choice with no new key (an input selects a layer that patches the product's features and adds the surface artifact and its install; the pattern is docs-only, 22dba15, resolved both ways with the Lemnos app), and the trim, which relinks a shared library exporting only what its users import (`trim = { users = [...] }`, 38aebbd).
  - Trim measured on 2026-11-06, on the HeliOS shared stack (HeliOS scratch d69c5870 configs, Raze): the four libraries went from 46.8 to 21.7 MB, and the whole stack is 38.9 MB plus a 1.2 MB libstd, against 45.4 MB static. Exports per library: Daedalus 31506 → 1568, Styx 23200 → 1014, Eidos 8891 → 38, Lemnos 1815 → 244. That matches the hand-made A/B to within 8 bytes per library. The relinks add about 4 minutes to the 14.5-minute cargo build.
  - LTO for a shared build: `lto = "linker"` (linker-plugin LTO through the building rustc's rust-lld, the profile's `lto` off, the trim relinking the bitcode). A HeliOS 50e9456c shared Raze build with it in place of the A/B's hand-made clang wrapper (2026-11-07, e74f847) built and passed `image:check`: bitcode objects, every library and program linked by rust-lld, the four trimmed libraries 21.80 MB against the wrapper's 21.83 MB (untrimmed 46.5 MB), `.img.xz` 46,557,764 B against 46,548,440 B.
  - Done when: a HeliOS Raze build makes both forms from one build file.
- **Surface and ABI checks for every dependency's C surface**
  - What: what was done for Daedalus (Daedalus a02eda4): declared exports, consumers' imports, plugin directories and exports, ABI range constants, one version per crate, and "not both".
  - Status: checks in (1873431, 717dbe9, bf45532, dce1b0b, 1e7d2c7, 77f38d4, f6a87ff), exercised on Daedalus only; not yet run against Lemnos's or Orion's C surfaces.
  - Done when: each dependency's surface (Lemnos, Orion, Styx, Eidos, Daedalus) is declared in HeliOS's config and its checks pass on a real build.
- **Combined build across workspaces**
  - What: one feature set per image, designed in docs/design/combined-surface-build.md (9b8a91b). Gaia's checks are in. Left: HeliOS's surface crate and its config.

Closed on 2026-11-06:
- Cold-build chain A/B: dropped. The host has not been quiet for four days, and a higher `make -j` can only speed up the 80 of 413 s that make sits at its 7-package cap.
- Module sources moving from Styx to Lemnos: not Gaia's work. Gaia only installs what Lemnos builds.
- Images sized to their content: nothing left on Gaia's side. The Raze slots disk is 140 MiB raw; skipping zero runs when flashing is Atlas's flasher work.

## Release mechanics

- **Changelog, version and tag**
  - What: move `[Unreleased]` to a version, set the workspace version (now `2.0.5-dev`), tag it, and create the GitHub release. The next one is a major version, because config v3 changes the build-file format.
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
  - What: the image job of HeliOS's `.github/workflows/gaia-builds.yml` (HeliOS 814e4893). The workflow's validate jobs already pass on GitHub's hosted runners on pushes to HeliOS's `dev`; pushes to `architecture-overhaul`, where HeliOS integrates, do not trigger them, and Gaia's own `Gaia action` workflow builds `imported-rootfs-minimal` on them (last green on 17c20df), so only HeliOS's image job needs the runner.
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

- Raze build time, under both of HeliOS's targets (2026-11-07, CPUs 6-31, -j13): one changed HeliOS crate on a warm build 3m58s at HeliOS 50e9456c (backend cargo group 162 s with fat LTO, `image:build` 53 s, assembly 12 s), target 5 minutes; cargo cold with Buildroot warm 17m11s at HeliOS 7dc8031f (the backend cargo group 10m53s on the critical path, no straggler once helios-peripherals was gone), target 20. A moved Eidos `dev` no longer reruns cargo (7371f2a). Not timed: a build with Buildroot cold too, at current HeliOS on a quiet host.
- Lemnos kernel module in the images: `[[image.rootfs.out_of_tree_modules]]` (c7f0a7f), `${source.<id>.path}` module paths (a0eeac9), version 2.0.3-dev for the floor (baf90c6). A HeliOS 50e9456c Raze image shipped `updates/lemnos.ko` (57,488 B stripped, about 15 KB xz, `vermagic` matching 7.2.9-v8-16k) and `image:check` passed.
- Size report for both modes, HeliOS 50e9456c Raze full, sent to HeliOS on 2026-11-07: static 48,769,176 B `.img.xz` (rootfs.erofs 41,345,024 B); shared with the trim 46,548,440 B (39,124,992 B), 2.2 MB smaller. Next to the board A/B (shared: +26% mirror tick p50, about +2 MB memory), static stays the default.

- Raze Mesa footprint: HeliOS's Raze build takes Atlas's Vulkan-only GPU layer (HeliOS 599c63cd, Atlas c1f7803), 3.1 MB less `.img.xz`.

- Raze build warnings: a full Raze build on 2026-11-06 printed none (the ccache warning went with the cache's move to the SSD).
- Noisy firmware warnings: many missing firmware files are one warning line, the list in the report: 96f0042.
- Docker containers keep Gaia's CPU affinity (`taskset` reaches the builds): 8c229a1.

- Config v3 enums: e411aef.
- `examples/sim-x86_64` and the app-layer fast path: a78b53a.
- G1, bootloaders at fixed disk offsets: 0b09e38.
- G2, U-Boot boot model in `image check`: 6ab2216. It checks a FIT's hashes; a FIT's RSA signature is not verified (not in scope until FIT signing is).
- G3, `uboot-env` transform: 56568df, with the `mkenvimage` and `fw_printenv` test in 1966f26. `copies` for the Luckfox's two env copies: 942746b.
- Packages: ipk (`[[image.assembly.packages]]`, deterministic, read back before publishing) and deb (`format = "deb"`); ipk-only builds with `[image] kind = "none"`; device sysroot checks (`[image.assembly.packages.device]`). Used by HeliOS for its SystemCore ipk.
- G4, riscv64 aliases and lp64d loader dirs: 8170082.
- G5, `gaia layer delta --max-window`: 69aecf2.
- G9, armv7 aliases and armhf closure: 3e51c14.
- G8, the UBI writer: 1a81221. The kernel attaching the Luckfox image on nandsim: 44fc94b. Atlas read the real `ubi.img`, so it has the volume layout.
- G10, CPU profile flags in cargo and Buildroot: 1966f26, with `cortex-a72` in a2aeb06. Per-board report, measured with Atlas's crates for every profile: docs/configuration-sources.md#cpu-profiles. HeliOS's Raze build uses `cortex-a76` on `architecture-overhaul` (HeliOS 48492ee5, whose commit message reports the full artifact build passing).
- G7, the gate validates and plans every board example and `examples/sim-x86_64`: `crates/core/app/tests/examples_boards.rs`. The examples that exist: ROCK 5C, ROCK 4D, Luckfox Pico Pro and K230D Zero in 006284a and 3d1f249; the cast receivers in a2aeb06 and db7b92a.
- `${source.<id>.commit_count}`: b47a9e5.
- Luckfox NAND slots, written and untested on board: 942746b, 720c211, 17c20df.
- Signing proof: real minisign and cosign sign and verify image outputs and registry entries in `scripts/ci.sh` and GitHub CI, with throwaway keys. cosign 3 had refused Gaia's `--output-signature`; it now writes an offline Sigstore bundle (`<file>.sigstore.json`).
