# TODO

## Next release (v3.0.0)

The feature list was frozen on 2026-10-27. Hardware support (group 2) is
the last addition: nothing else joins this release. The release is Gaia
3.0.0, tagged once this whole push is done (user decision, 2026-10-30).
Every item says what it is, why, when it is done, and what blocks it.
Items the user pinned for a later release are in "Pinned for later" below.

### 1. Freeze items still open

- **sim-x86_64 example and the app-layer fast path**
  - What: an x86_64 Buildroot example built as layers. Changing only the app rebuilds only the app layer.
  - Why: a target anyone can build and test without a board, and the fast path every product relies on.
  - Done when: `gaia run` on the example passes `image check`, and a second run after an app-only change reuses the base and platform layers. Record both run times in the changelog.
  - Blocked by: the user's approval to download the x86_64 Buildroot sources.
- **Config v3: real enums**
  - What: read `checkpoints.backend`, `checkpoints.sign_kind`, `image.kind_layer`, `pi_model` and the transform and assembly `kind`s as enums in v3 files, so a typo is a parse error at its line that lists the allowed values (section 4 of `docs/design/config-v3.md`). The rest of v3 and `gaia migrate` landed on 2026-10-31.
  - Blocked by: nothing.

### 2. Hardware support (G1–G9 and board bring-up)

Small targets ship exactly two slots (A/B) plus a data partition. Smaller is better, because it lets the boards use cheaper flash.

- **G1: bootloaders at fixed disk offsets**
  - What: partition `offset` and `table = false`.
  - Why: Rockchip's eMMC/SD boot ROM reads the loader at sector 64 and U-Boot at 8 MiB.
  - Done when: the assembly tests (`tests_disk_offsets`) and validation tests pass in the gate.
  - Status: done in 0b09e38.
- **G2: U-Boot boot model in `image check`**
  - What: check extlinux.conf, boot.scr and FIT boot trees: the kernel arch, the DTBs they name, the initrd and the `roothash=` in the command line.
  - Why: `image check` knows only the Raspberry Pi firmware's boot config.
  - Done when: tests cover extlinux and FIT trees, both good and broken.
  - Status: done in 6ab2216. A FIT signed with RSA is not verified; only its hashes are.
- **G3: `uboot-env` transform**
  - What: a U-Boot environment image (CRC32, optional redundant copy) holding the A/B variables (`boot_slot`, `bootcount`, `bootlimit`, `altbootcmd`).
  - Why: A/B trial boots on U-Boot boards, which have no `tryboot`. Atlas's updater writes the same variables.
  - Done when: `fw_printenv` reads the image back the same in a test, where installed.
  - Status: done. The transform landed in 56568df; the commit adding CPU profiles adds the test: `mkenvimage` writes byte-identical images, plain and redundant, and `fw_printenv` reads the variables back from both. It runs here (u-boot-tools 2026.04) and in GitHub CI, which now installs u-boot-tools, and skips with a visible line elsewhere.
- **G4: riscv64 polish**
  - What: GPT aliases `linux-usr-riscv64`, `linux-root-verity-riscv64` and `linux-usr-verity-riscv64`, plus the riscv64 lp64d loader paths in the runtime-library check.
  - Why: the K230D uses Raze's verity layout.
  - Done when: tests cover the aliases and a riscv64 closure.
  - Status: done in 8170082.
- **G5: `gaia layer delta --max-window`**
  - What: bound the memory a board needs to apply an update. zstd gives a delta a window over its whole base tar, so deltas that would need more than the cap are not made, and whole layers use the capped window.
  - Why: a board needs about window-sized RAM to apply a file. That is 115 MiB for Raze's base layer, and a K230D has 128 MB in total.
  - Done when: a test applies a capped whole layer under `--memory`. The Raze dev.800→801 sizes with the cap are in the changelog.
  - Status: done in 69aecf2.
- **G9: armv7 (Flame) polish**
  - What: GPT aliases `linux-usr-arm`, `linux-root-verity-arm` and `linux-usr-verity-arm`; the armhf loader and its library dirs in the runtime-library check; and tests with 32-bit ARM ELF fixtures. Rust's `armv7-unknown-linux-gnueabihf` and `musleabihf` targets already map to `arm`.
  - Why: the user wants full support on Flame's Cortex-A7, the same as the other targets.
  - Done when: tests cover the aliases and an armhf closure.
  - Status: done in 3e51c14.
- **G8: UBI images for SPI-NAND**
  - What: a native UBI writer, with no mtd-utils download. It writes one volume per slot (EROFS with verity on ubiblock, layered as on Raze) plus a data volume. It also lays out Rockchip's SPI-NAND idblock and K230 NAND images.
  - Why: the K230D boards, Flame and the Rockchip L variants boot from SPI-NAND.
  - Done when: the image attaches with `nandsim` and `ubiattach` and its volumes match, where those tools exist; otherwise a byte-level test of the format. Atlas has the volume layout.
  - Status: the writer is done in 1a81221, with byte-level read-back tests. nandsim needs root here and mtd-utils is not installed, so the first kernel attach is on a board. Not done: Rockchip's SPI-NAND idblock and the K230 NAND image layout (these need the vendor SDKs' exact formats), and telling Atlas the volume layout once a board config fixes it.
  - Blocked by: nothing external for the formats: fetch the K230 SDK and rkbin (approved on 2026-10-30, not fetched yet) and read them from there. A kernel attach needs a board, or the user's yes to `sudo modprobe nandsim`.
- **G10: per-board CPU profile**
  - What: a `[cpu]` table a board layer sets (`profile = "cortex-a76"`, ...). It adds `-C target-cpu`/`-C target-feature` and the C compiler's `-mcpu` to every Rust artifact built for that architecture, and selects the CPU in Buildroot (`BR2_cortex_a76`, ...). Profiles: CM5 `cortex-a76`; RK3588S/RK3582 `cortex-a76.cortex-a55`; RK3576 `cortex-a72.cortex-a53` (the RK3576's cores are A72 and A53, not A76); RK3566 `cortex-a55`; RK3562 `cortex-a53` (its cores are A53); Flame `cortex-a7` with NEON and VFPv4; K230D `xuantie-c908` (RVV 1.0 and Zba/Zbb/Zbs; rustc has no C908 model).
  - Why: today the aarch64 builds target generic armv8-a, so the A76's dotprod, fp16, CRC32 and crypto instructions are used only where a crate detects them at run time (requested 2026-10-31 through HeliOS).
  - Done when: the flags reach cargo and Buildroot in tests, HeliOS's backend builds with the CM5 profile, and each board's flags and any crate that breaks are reported to HeliOS.
  - Status: written. Tests compile a dotprod-only crate for aarch64 (it fails without the profile and builds with `cortex-a76`, keeping the crate's own `--cfg`). The K230 SDK's Buildroot 2024.02.1 has `BR2_RISCV_ISA_RVV`. Left: HeliOS's backend built with the CM5 profile, and the per-board report.
  - Blocked by: nothing.
- **G6: per-board size budget**
  - What: an error-level check on slot and image bytes against a per-board budget, plus a report of the smallest flash part each board could ship with.
  - Why: two slots plus data has to fit the cheapest flash. A Raze slot is about 43 MB layered today.
  - Status: done for raw NAND in 1a81221. A UBI image's `flash_size` fails the build when the volumes do not fit, and every build reports `minimum_flash_bytes`. On disk images, an image larger than its partition already fails. Left: a whole-device budget for disk images (eMMC/SD), so a disk larger than the board's part fails with the smallest size it needs, and the smallest-flash numbers per board for the PCB session.
  - Done when: a real HeliOS build passes and an oversized one fails. The smallest-flash numbers go to the PCB session.
  - Blocked by: a board config per board (G7) to take the numbers from, and a real HeliOS build run before the disk check can fail builds.
- **G7: board examples and docs**
  - What: `examples/buildroot-k230d`, `examples/buildroot-rk3588s` and `examples/buildroot-rv1106`, plus their docs.
  - Why: each new feature shown in a full board config.
  - Done when: both examples validate and plan in the gate, and build where the sources are present.
  - Blocked by: G8's idblock and K230 NAND layouts (the examples need their boot media), and the vendor sources for the build step. G1–G5 and G9 are done.
- **K230D BSP (HVS_U01, HVS_I02)**
  - What: Canaan's Linux-only SDK with Linux on the big C908 (CPU1, the only core with RVV), vendor U-Boot and OpenSBI, and SPI-NAND boot. Size the media memory reservation from Styx's estimate of 20–25 MB.
  - Why: the smart USB camera and the Inferno host.
  - Done when: a board boots slot A, falls back from a bad slot B, and `grep isa /proc/cpuinfo` shows `v`.
  - Blocked by: the boards (PCB). The user approved the SDK download on 2026-10-30.
- **Rockchip BSP: RK3588S/RK3582 (HVS_MH01) and RK3576 (HVS_MM01)**
  - What: Rockchip's vendor 6.1 kernel with rkisp, rkcif, RGA, MPP (rkvenc) and RKNN, but no rkaiq and no librga.
  - Boot chain: mainline U-Boot, TF-A, and the rkbin DDR blob.
  - rkbin licence (checked 2026-10-30 at rkbin 3e288fe): Rockchip grants a non-exclusive licence to use, copy and distribute. It forbids decompiling or reverse engineering, and the copyright notices must stay. Ship rkbin's `LICENSE` in the image's licence directory and in each export. DDR blobs for RK3588, RK3576, RK3566 and RK3562 are in `bin/rk35`.
  - GPU: libmali with Vulkan (Daedalus).
  - eMMC (E) and SPI-NAND (L) variants.
  - Why: the CM5-format camera modules.
  - Done when: each module boots A/B from both storage variants, and Styx and Daedalus see their devices.
  - Blocked by: the boards. The user approved the downloads on 2026-10-30.
- **Mainline RK3588S track**
  - What: a second kernel option for the RK3588S.
  - Why: it is needed once rkisp2 and rkvenc are upstream.
  - Done when: the module boots mainline with capture working.
  - Blocked by: rkisp2 and rkvenc landing upstream, and a board.
- **Flame BSP (RV1106G2, HVS_F02)**
  - What: an RV1106 vendor BSP (armv7) with SPI-NAND boot. It gets the same support as the other targets: EROFS and verity slots, layers, env sets and size reporting.
  - Why: Flame.
  - Done when: it boots A/B.
  - Blocked by: the board, and the user's approval to download the RV1106 SDK (not covered by the 2026-10-30 approval).
- **Low-cost tier: RK3566 and RK3562 (HVS_ML01)**
  - What: both chips (the user said "make both"). The vendor 6.1 BSP and boot chain are shared with the other Rockchip modules. RK3562 boards use 256–512 MB DDR3L, and 16-bit DDR3 boot on the RK3562 is unproven.
  - Why: the low-cost module tier.
  - Done when: each chip's module boots A/B from SPI-NAND and from eMMC.
  - Device tree inputs (from the PCB session, 2026-10-31; more pin changes may follow): the HVS_ML01 J1 GPIO map is in `HW-Helios-Platform/projects/HVS_ML01_DEV01/README.md` ("Software / device tree: J1 GPIO map"). The CM5-header SPI0 (J1 GPIO7–11) uses the RK3562 SPI0 **M1** pin group, not M0. The DDR3L byte-lane DQ swaps are in `docs/DDR-MATCHING.md` §18, for the DDR init. Read both again when writing the board's DTS, not this copy.
  - Blocked by: the boards. Downloads approved on 2026-10-30.

### 3. Release mechanics

- **Changelog and version**
  - What: move `[Unreleased]` to `[3.0.0]` and set the workspace version to `3.0.0`. The version is major because config v3 changes the build-file format.
  - Done when: `cargo package --workspace` and the gate pass at that version.
  - Blocked by: groups 1 and 2, or the user choosing to release without some of them.
- **Tag v3.0.0**
  - Blocked by: groups 1 and 2, and the user's own go-ahead in chat.
  - Done when: the tag is pushed, the GitHub release exists, and HeliOS, PV and Atlas pin `v3.0.0`.

## Pinned for later

The user pinned these on 2026-10-30. They are not part of 3.0.0.

- **Signing proof (minisign and cosign)**
  - What: sign and verify an export with real minisign and cosign in the gate.
  - Why: today the signing paths are tested only against Gaia's own reading of the formats.
  - Done when: `scripts/ci.sh` and GitHub CI sign and verify with both tools, and skip with a visible line when a tool is missing.
  - Blocked by: the user's approval to install minisign and cosign.
- **Registry backend**
  - What: the registry cache pushed to a real registry.
  - Why: CI and boards share built layers.
  - Done when: a push and pull round trip runs against the chosen registry.
  - Blocked by: the user's decision on GHCR (yes or no) and who owns it. No real-registry push until then.
- **`gaia run` CI on the self-hosted runner**
  - What: the HeliOS workflow (HeliOS dev 814e4893) on a registered `gaia` runner. Gaia's own `Gaia action` workflow already builds `imported-rootfs-minimal` green on GitHub's hosted runners (last on cd9ad15), so it needs no self-hosted runner.
  - Why: real image builds per change, not just unit tests.
  - Done when: the HeliOS workflow goes green on GitHub.
  - Blocked by: the user registering the runner, adding the `EIDOS_READ_TOKEN` secret and setting the fork-PR policy.
- **Atlas's signing key**
  - What: the key Atlas signs updates with, and where it lives.
  - Why: signed updates on boards.
  - Done when: Atlas verifies a Gaia-signed export on a board.
  - Blocked by: the user, and the signing proof above.

## Backlog

Cut from this release and staying cut: emulated QEMU boot, TUI cleanup, deb
packaging niceties.
