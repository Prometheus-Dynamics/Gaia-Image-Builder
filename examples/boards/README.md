# Board examples

Images for four dev boards, each with two root slots (A/B) and a data
partition. They are **written, untested on board**: each builds, and its
images are checked structurally (partition and UBI layouts, `image:check`),
but none has booted on its board yet.

| Board | SoC | Boot medium | Kernel, U-Boot | Config |
| --- | --- | --- | --- | --- |
| Radxa ROCK 5C | RK3588S2 (A76+A55) | SD / eMMC, GPT | mainline Linux 6.19, U-Boot 2026.01, TF-A 2.12 | [rock-5c](rock-5c/build.toml) |
| Radxa ROCK 4D | RK3576 (A72+A53) | SD / eMMC, GPT | mainline Linux 6.19, U-Boot 2026.01, rkbin BL31 | [rock-4d](rock-4d/build.toml) |
| Luckfox Pico Pro | RV1106G2 (A7) | 128 MB SPI-NAND, UBI | Luckfox SDK 5.10 kernel and U-Boot | [luckfox-pico-pro](luckfox-pico-pro/build.toml) |
| BPI-CanMV-K230D-Zero | K230D (C908, RVV 1.0) | SD (TF only), GPT | ruyisdk 6.6 kernel, SDK U-Boot 2022.10, OpenSBI 1.4 | [k230d-zero](k230d-zero/build.toml) |

Appliance images, single-slot with a read-only EROFS root, also written,
untested on board ([cast-receiver](cast-receiver/README.md): AirPlay + DLNA
receiver on HDMI, Raspberry Pi 6.12 kernel, one external tree for both):

| Board | SoC | Boot medium | Video decode | Config |
| --- | --- | --- | --- | --- |
| Raspberry Pi CM4 | BCM2711 (A72) | eMMC / SD, MBR | VideoCore H.264 (V4L2) | [cast-receiver/cm4](cast-receiver/cm4/build.toml) |
| Raspberry Pi 5 / 500 | BCM2712 (A76) | SD, MBR | FFmpeg H.264 on the A76s | [cast-receiver/pi5](cast-receiver/pi5/build.toml) |

Each sets its `[cpu]` profile (see `docs/configuration-sources.md#cpu-profiles`),
so Rust artifacts and the Buildroot toolchain target the board's cores.

## Loaders for USB flashing

Each build publishes the vendor loader next to its images, where
`rkdeveloptool`, `k230-flash` and Atlas look for it:

- ROCK 5C: `rk3588_spl_loader_v1.18.113.bin` (`RK3588MINIALL.ini` of the rkbin Buildroot pins, `f43a462`)
- ROCK 4D: `rk3576_loader_v1.13.109.bin` (rkbin `RK3576MINIALL.ini`)
- Luckfox Pico Pro: `rv1106_download_v1.15.108.bin` (the SDK's rkbin)
- K230D Zero: `loader_mmc.bin`, the burner: the SDK's U-Boot built with
  `k230d_canmv_burntool_defconfig` (plain `u-boot.bin`, as Canaan's burning
  guide describes). It links at `0x80300000`, so give `k230-flash`
  `-la 0x80300000` rather than its default `0x80360000`.

The Luckfox `idblock.img` and download loader pair: both come from the
SDK rkbin's `RV1106MINIALL.ini` (`rv1106_download_v1.15.108.bin` and
`rv1106_idblock_v1.15.102.img`, which `idblock.img` matches byte for byte)
and carry the same DDR init, `ddr-v1.15-306b9977f5`. The idblock's SPL is
the one the SDK's `make.sh --spl-new` builds from its U-Boot, as the SDK
itself does.

## Layouts

**ROCK 5C and ROCK 4D** (GPT, SD or eMMC): `u-boot-rockchip.bin` (idbloader
and U-Boot) at 32 KiB outside the table; an empty `uboot-env` partition at
16 MiB with U-Boot's environment type GUID, so the built-in environment
boots; `rootfs-a` (bootable, holds `/boot/extlinux/extlinux.conf`),
`rootfs-b` (empty), `data`.

**Luckfox Pico Pro** (SPI-NAND, 128 KiB blocks, 2 KiB pages), the
`mtdparts` in `env.img`:

| Offset | Size | Image |
| --- | --- | --- |
| 0 | 256K | `env.img` (two 128K copies of the U-Boot environment; see [NAND slots](#nand-slots)) |
| 256K | 256K | `idblock.img` |
| 512K | 512K | `uboot.img` |
| 1M | 6M | `boot.img` (FIT: zImage, dtb, resource; 4.4 MiB) |
| 7M | 6M | `boot_b` (empty) |
| 13M | rest | `ubi.img`: `rootfs_a` (squashfs, via `ubiblock`), `rootfs_b`, `data` (autoresize) |

Flash `ubi.img` with `ubiformat`, not as raw pages (see
`docs/configuration-assembly.md#ubi-images`).

**K230D Zero** (GPT, SD): the SPL at 1 MiB and 1.5 MiB, the U-Boot
environment at 1.875 MiB and 3.5 MiB, U-Boot at 2 MiB, all outside the
table, as the SDK's `genimage.cfg` places them; `boot` (ext4, partition 1:
OpenSBI, kernel, dtb) at 30 MiB; `rootfs-a` at 128 MiB as partition 2, as in
the SDK image; `rootfs-b` (empty); `data`. The example's
`assets/post-image.sh` makes those boot files with the SDK's tools (its
gzip, firmware head and U-Boot's `mkimage`); the environment is the SDK's
`default.env`, whose `blinux` loads the kernel from partition 1, and the
dtb's `bootargs` mount `mmcblk0p2`.

## Sources

- ROCK 5C and 4D: Buildroot 2026.02.3. The ROCK 4D also pins rkbin at
  `3e288fe`, for BL31 and the DDR init, which Buildroot's mainline TF-A
  2.12 lacks for the RK3576.
- Luckfox Pico Pro: Buildroot 2026.02.3 with the Luckfox SDK
  (`LuckfoxTECH/luckfox-pico` at `824b817`): its uClibc toolchain, its
  kernel through `override_srcdirs`, its U-Boot built by the post-image
  script with the SDK's `make.sh`.
- K230D Zero: the K230 Linux SDK v0.6.1's Buildroot tree. Check the SDK out
  as `k230d-zero/sdk` and prepare it once:

  ```bash
  make -C examples/boards/k230d-zero/sdk sync
  ```

  `sync` downloads Buildroot 2024.02.1 (sha256 `12eda6b5…4a9dba` here; the
  SDK has no hash for it) and copies the SDK's overlay onto it. The Xuantie
  GCC 10.4 toolchain is an archive source with its sha256. For current
  hosts (GCC 16, wget2), the example takes the system ccache (Buildroot's
  own needs host-cmake 3.28, which does not build with GCC 16), builds the
  host tools as C17 (`assets/buildroot/external.mk`) and drops wget's
  `--passive-ftp`.

## What was checked

Built on 2026-10-31 (Fedora 44 host, GCC 16), each run passing
`image:check`:

- ROCK 5C and 4D: the GPT entries, `u-boot-rockchip.bin` byte for byte at
  32 KiB (`RKNS` header), `extlinux.conf` with the kernel and dtb in
  `rootfs-a`, the loader's `LDR` header.
- Luckfox Pico Pro: `ubi.img` read back (layout volume, `rootfs_a` holding
  the squashfs, `data` autoresize), `env.img` CRC and `mtdparts`, the
  `boot.img` FIT hashes; the kernel module check on the 5.10 `zImage`
  (27 modules).
- K230D Zero: the GPT entries, the SPL, environment and U-Boot copies byte
  for byte at their offsets (`K230` firmware heads), the environment CRC,
  `boot.ext4` contents (root-owned), `loader_mmc.bin` built from the
  burntool configuration (USB DFU for MMC and SPI-NAND).

## NAND slots

The Luckfox Pico Pro's slot lives in its environment, and the vendor U-Boot is
not patched. Written from the SDK's U-Boot and kernel source and **untested on
board**.

The env partition holds two 128K copies. U-Boot reads the first (offset 0) and,
when its CRC fails, the second (offset 128K). It takes `mtdparts`, `sys_bootargs`,
`bootcmd` and `gaia_trial` from the copy it read. The build publishes the four
possible copies, 128K each, to write unchanged:

| Image | Boots | Kernel partition names | Kernel command line |
| --- | --- | --- | --- |
| `env-a.img` | `boot` at 1M, `rootfs_a` (`ubiblock0_0`) | mtd3 `boot`, mtd4 `boot_b` | `gaia.slot=a` |
| `env-b.img` | `boot` at 7M, `rootfs_b` (`ubiblock0_1`) | mtd3 `boot_a`, mtd4 `boot` | `gaia.slot=b` |
| `env-a-trial.img` | as `env-a.img`, once | as `env-a.img` | `gaia.slot=a gaia.trial=1` |
| `env-b-trial.img` | as `env-b.img`, once | as `env-b.img` | `gaia.slot=b gaia.trial=1` |

U-Boot's `boot_fit` only reads the partition named `boot`, so the slot is chosen
by which partition has that name: the running slot's is `boot`, and the other
is named for the slot it holds (`boot_a` or `boot_b`). `env`, `ubi` and every
mtd number stay the same.

`slot-bundle.tar` holds `boot.img`, `rootfs.squashfs` and all four environments.
The root is read-only, and UBI volume `data` is mounted at `/data` as UBIFS, which
the kernel formats on the first mount since the volume starts empty. It is shared
by both slots, so a commit image kept there survives the trial's reboot.

The SDK disables the watchdog in `rv1106.dtsi`; `post-image.sh` enables it in the
board's DTS, so `/dev/watchdog` exists. `WATCHDOG_NOWAYOUT` is off, and nothing
starts the watchdog by default.

The image imports Atlas's `devices/luckfox-pico-pro/gaia/device.toml`.
`/usr/sbin/atlas-ubi nand-slot` carries out the steps below, and
`/etc/init.d/S00nand-slot` runs its trial check and starts the watchdog early on a
trial boot. It does nothing on other boots. `/usr/sbin/board-lite` identifies the
board to Atlas, read-only: `GET :5899/.well-known/pd-device`, and mDNS for
`_pd-device._tcp`. Nothing on the image can start an install remotely yet: it has
no SSH or USB console.

To install to the other slot, say B while A is committed:

1. Write mtd4 (`boot_b`) and volume `rootfs_b`.
2. Erase the first block of mtd0 (`env`) and write `env-b-trial.img` at offset
   0. The second copy still says A; a torn write fails the CRC and boots A.
3. Reboot. U-Boot sees `gaia_trial`, erases the first copy, and boots B. Any
   later boot reads the second copy, A, so a hang or a failed FIT falls back. A
   failed FIT resets by itself; a hung kernel needs the watchdog, or a power
   cycle. `panic=5` covers a panic.
4. If B is healthy, commit: write `env-b.img` to offset 0, then to offset 128K,
   in that order, each after erasing its block. Never erase the second copy
   while the first is invalid, since with neither copy U-Boot has no partition
   table. To reject B, reboot without writing anything.

On a trial boot, if the first copy still has a valid CRC, U-Boot's erase failed
and the next boot would retry the trial: erase it from Linux and report it.
Refuse to install if `MEMGETBADBLOCK` says either env block is bad, because
U-Boot skips a bad block and raw offsets on mtd0 would then no longer match.
Don't use `fw_setenv`: its redundant format has a flags byte this U-Boot doesn't
read, and it would not keep the write order above.

To check on a board first:

- `mtd list` at the U-Boot prompt names `spi-nand0`.
- A trial of each slot, killed in Linux, comes back on the committed slot.
- `/data` is formatted and mounted on the first boot.
- A trial nobody commits resets at about 600 s plus the watchdog timeout, and a
  commit stands the watchdog down.
- `/proc/cpuinfo` has a non-zero `Serial` (`board-lite` falls back to the device
  tree, then the MAC).
- `atlas ls` on a laptop on the same LAN lists the board, which also shows that
  multicast reaches `eth0` and nothing else holds UDP 5353. `board-lite` logs
  nothing if it can't advertise, because its init script discards its output.

## Still open

- Boot each image on its board; read the RK3576 maskrom USB product id with
  `lsusb` on the ROCK 4D.
- Slot switching on the ROCK and K230 boards: their images boot slot A;
  selecting B (U-Boot environment or `extlinux` labels) is for the updater.
- Luckfox NAND slots are written, untested on board; see below.
- dm-verity on the read-only slots: the Luckfox SDK kernel has no
  `CONFIG_DM_VERITY` yet.
