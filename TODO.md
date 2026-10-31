# TODO

## Next release (v2.0.0)

The feature list was frozen on 2026-10-27. Hardware support (group 2) is
the last addition: nothing else joins this release. Every item says what it
is, why, when it is done, and what blocks it.

### 1. Freeze items still open

- **sim-x86_64 example and the app-layer fast path**
  - What: an x86_64 Buildroot example built as layers. Changing only the app rebuilds only the app layer.
  - Why: a target anyone can build and test without a board, and the fast path every product relies on.
  - Done when: `gaia run` on the example passes `image check`, and a second run after an app-only change reuses the base and platform layers. Record both run times in the changelog.
  - Blocked by: the user's approval to download the x86_64 Buildroot sources.
- **Config v3 and `gaia migrate`**
  - What: one token syntax, a `[cache]` table, outputs at the top level, real enums instead of strings, and no explicit `patch = true`. v2 files keep loading, with a deprecation warning.
  - Why: v2's inconsistencies are in every HeliOS, PV and Atlas build file.
  - Done when: `gaia migrate` rewrites each example and HeliOS's and PV's build files to v3, and the v2 and v3 files resolve to the same spec fingerprint (a test). The docs and `docs/reference/config-keys.md` describe v3.
  - Blocked by: nothing. Confirm the design with HeliOS before writing it.
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
  - What: the HeliOS workflow (HeliOS dev 814e4893) and the `Gaia action` workflow on a registered `gaia` runner.
  - Why: real image builds per change, not just unit tests.
  - Done when: both workflows go green on GitHub.
  - Blocked by: the user registering the runner, adding the `EIDOS_READ_TOKEN` secret and setting the fork-PR policy.
- **API cleanup: last duplicates**
  - What: merge the remaining small duplicates: `cstring`, `write_marker` and the `execution_backend` copies.
  - Why: the cleanup pass covers every duplicate.
  - Done when: one copy of each remains and the gate passes.
  - Blocked by: nothing.

### 2. Hardware support (G1–G8 and board bring-up)

Small targets ship exactly two slots (A/B) plus a data partition. Smaller is better, because it lets the boards use cheaper flash.

- **G1: bootloaders at fixed disk offsets**
  - What: partition `offset` and `table = false`.
  - Why: Rockchip's eMMC/SD boot ROM reads the loader at sector 64 and U-Boot at 8 MiB.
  - Done when: the assembly tests (`tests_disk_offsets`) and validation tests pass in the gate.
  - Blocked by: nothing.
- **G2: U-Boot boot model in `image check`**
  - What: check extlinux.conf, boot.scr and FIT boot trees: the kernel arch, the DTBs they name, the initrd and the `roothash=` in the command line.
  - Why: `image check` knows only the Raspberry Pi firmware's boot config.
  - Done when: tests cover extlinux and FIT trees, both good and broken.
  - Blocked by: nothing.
- **G3: `uboot-env` transform**
  - What: a U-Boot environment image (CRC32, optional redundant copy) holding the A/B variables (`boot_slot`, `bootcount`, `bootlimit`, `altbootcmd`).
  - Why: A/B trial boots on U-Boot boards, which have no `tryboot`. Atlas's updater writes the same variables.
  - Done when: `fw_printenv` reads the image back the same in a test, where installed.
  - Blocked by: nothing.
- **G4: riscv64 polish**
  - What: GPT aliases `linux-usr-riscv64`, `linux-root-verity-riscv64` and `linux-usr-verity-riscv64`, plus the riscv64 lp64d loader paths in the runtime-library check.
  - Why: the K230D uses Raze's verity layout.
  - Done when: tests cover the aliases and a riscv64 closure.
  - Blocked by: nothing.
- **G5: `gaia layer delta --max-window`**
  - What: cap zstd's window when making a delta.
  - Why: a board needs about window-sized RAM to apply a delta. That is 115 MiB for Raze's base layer, and a K230D has 128 MB in total.
  - Done when: a test applies a capped delta with `zstd --long=<cap>`. The Raze dev.800→801 sizes with the cap are in the changelog.
  - Blocked by: nothing.
- **G8: UBI images for SPI-NAND**
  - What: a native UBI writer, with no mtd-utils download. It writes one volume per slot (EROFS with verity on ubiblock, layered as on Raze) plus a data volume. It also lays out Rockchip's SPI-NAND idblock and K230 NAND images.
  - Why: the K230D boards, Flame and the Rockchip L variants boot from SPI-NAND.
  - Done when: the image attaches with `nandsim` and `ubiattach` and its volumes match, where those tools exist; otherwise a byte-level test of the format. Atlas has the volume layout.
  - Blocked by: nothing for the writer. The board run is gated on the boards.
- **G6: per-board size budget**
  - What: an error-level check on slot and image bytes against a per-board budget, plus a report of the smallest flash part each board could ship with.
  - Why: two slots plus data has to fit the cheapest flash. A Raze slot is about 43 MB layered today.
  - Done when: a real HeliOS build passes and an oversized one fails. The smallest-flash numbers go to the PCB session.
  - Blocked by: a real build per board, run before the check can fail builds.
- **G7: board examples and docs**
  - What: `examples/buildroot-k230d` and `examples/buildroot-rk3588s`, plus their docs.
  - Why: each new feature shown in a full board config.
  - Done when: both examples validate and plan in the gate, and build where the sources are present.
  - Blocked by: G1–G6 and G8.
- **K230D BSP (HVS_U01, HVS_I02)**
  - What: Canaan's Linux-only SDK with Linux on the big C908 (CPU1, the only core with RVV), vendor U-Boot and OpenSBI, and SPI-NAND boot. Size the media memory reservation from Styx's estimate of 20–25 MB.
  - Why: the smart USB camera and the Inferno host.
  - Done when: a board boots slot A, falls back from a bad slot B, and `grep isa /proc/cpuinfo` shows `v`.
  - Blocked by: the boards (PCB) and the user's own approval of the SDK download.
- **Rockchip BSP: RK3588S/RK3582 (HVS_MH01) and RK3576 (HVS_MM01)**
  - What: Rockchip's vendor 6.1 kernel with rkisp, rkcif, RGA, MPP (rkvenc) and RKNN, but no rkaiq and no librga.
  - Boot chain: mainline U-Boot, TF-A, and the rkbin DDR blob, with its licence recorded.
  - GPU: libmali with Vulkan (Daedalus).
  - eMMC (E) and SPI-NAND (L) variants.
  - Why: the CM5-format camera modules.
  - Done when: each module boots A/B from both storage variants, and Styx and Daedalus see their devices.
  - Blocked by: the boards and the user's own approval of the BSP and rkbin downloads.
- **Mainline RK3588S track**
  - What: a second kernel option for the RK3588S.
  - Why: it is needed once rkisp2 and rkvenc are upstream.
  - Done when: the module boots mainline with capture working.
  - Blocked by: rkisp2 and rkvenc landing upstream, and a board.
- **Flame BSP (RV1106G2, HVS_F02)**
  - What: an RV1106 vendor BSP with SPI-NAND boot.
  - Why: Flame.
  - Done when: it boots A/B.
  - Blocked by: the board and download approval.
- **Low-cost tier**
  - What: RK3566 or RK3562 for HVS_ML01.
  - Blocked by: the user's decision. HeliOS recommends the RK3566; spend no effort on the RK3562 until decided.

### 3. Release mechanics

- **Changelog and version**
  - What: move `[Unreleased]` to `[2.0.0]` and set the workspace version to `2.0.0`.
  - Done when: `cargo package --workspace` and the gate pass at that version.
  - Blocked by: groups 1 and 2, or the user choosing to release without some of them.
- **Release automation**
  - What: a workflow that, on a `v*` tag, builds the release binaries, writes checksums and signs them.
  - Done when: a dry run on a test tag in a fork or a draft release produces every file.
  - Blocked by: the signing proof above.
- **Tag v2.0.0**
  - Blocked by: the user's own go-ahead in chat.
  - Done when: the tag is pushed, the GitHub release exists, and HeliOS, PV and Atlas pin `v2.0.0`.

## Backlog

Cut from this release and staying cut: emulated QEMU boot, TUI cleanup, deb
packaging niceties.
