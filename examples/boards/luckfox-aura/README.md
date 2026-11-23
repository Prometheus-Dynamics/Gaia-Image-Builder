# Luckfox Aura (RV1126B)

**No bootable loader is built.** The `luckfox-aura-bench` product plans,
but its build stops at the bootloader step, before anything is compiled:
"the pinned U-Boot cannot make `u-boot-rockchip.bin`", the file the family
writes at sector 64. This U-Boot does compile (built once, before that
check existed; the build then failed when the file was not there to
copy). No image of this product boots. The board files are untested on a
board.

## Why there is no loader

The RV1126B has no mainline U-Boot port, so the family uses Rockchip's
own U-Boot (next-dev, `1c535d65`). That tree packs its loader with
Rockchip's `make.sh` and rkbin's closed x86 tools (`boot_merger`), which
this example does not run. Gaia builds from source, and building the
loader from source is what is missing:

- Buildroot's U-Boot build expects `u-boot-rockchip.bin` from binman, and
  this tree does not use binman for the RV1126B.
- rkbin's `RKBOOT/RV1126BMINIALL.ini` asks for the new IDB header
  (`NEWIDB=true`, header v2). This tree's `mkimage` has no `rv1126b` entry
  for `-T rksd`, only `rv1126` with header v1.

A loader made with guessed header values would look complete and still
be unproven at the board's first stage. So it is not made until it can be
checked on a board.

## What does build

- The family's U-Boot, with one source patch
  (`../rv1126b/assets/patches/u-boot-next-dev-cmd-process.patch`:
  `cmd_process` declared with the return type of its definition, which
  GCC 13 and later require under the tree's `-Werror`).
- The U-Boot environment at a fixed offset: the top 64 KiB of the
  `uboot-env` partition (`CONFIG_ENV_OFFSET=0x1010000`), one copy, on
  mmc 0. This tree cannot find the partition by its type GUID, and keeps
  the device and the redundant copy in C headers. Whether mmc 0 is the
  microSD is UNVERIFIED.
- The board's U-Boot defconfig (`rv1126b-env-sdcard`) and DDR blob are in
  `board.toml`, since they depend on the board.

## What a from-source loader needs

This is the proof to run once an Aura is on the bench. It is short work
then, not research.

The image is the v2 IDB at sector 64. Mainline U-Boot's
`tools/rkcommon.c` writes this header for `rk3506`, `rk3528`, `rk3568`,
`rk3576` and `rk3588`, and is the model:

- `header0_info_v2`: magic `RK_MAGIC_V2` (`"RKNS"`); `size_and_nimage` =
  number of images in bits 31:16 and the offset of the hash field in
  4-byte words in bits 15:0 (mainline: `(2 << 16) + 384`); `boot_flag`
  bits 3:0 = hash type (mainline: SHA-256); a SHA-256 of the header up to
  its hash field.
- Two `image_entry` records (init, then boot). Each has `size_and_off` =
  sector count in bits 31:16 and sector offset in bits 15:0 (the first
  image starts at sector 4), plus `address`, `counter` = 1, 2 and a
  SHA-256 of the image.
- An `spl_infos[]` entry for `rv1126b`. In mainline's table the v2 chips
  all use spl_hdr `"RK35"`, and their `spl_size` is the chip's SRAM for
  the init stage less 4 KiB.

Where the RV1126B may differ from mainline's rk3588 path. Each point is
read from rkbin's ini, not from a board:

- **Load address.** The ini sets `LOADER2_PARAM LOAD_ADDR=0x4fe00000` for
  the boot image. Mainline writes `address = 0xFFFFFFFF` for both images.
  If the RV1126B boot ROM reads this field, the second entry needs
  `0x4fe00000`, or wherever this U-Boot's SPL links.
- **Boot parameters.** The ini has `BOOT0_PARAM WORD_0=0x434D4D45` (the
  ASCII `"EMMC"`) and `WORD_3=0x1`. RK3588's ini has only an all-zero
  `BOOT1_PARAM`. Where `boot_merger` writes these words in the header is
  not known without its source. They may be what makes the ROM take the
  card.
- **Init stage size.** `spl_size` for the RV1126B (its SRAM for the DDR
  init) is not in mainline. The DDR blob's size is a lower bound.
- **Second stage.** Rockchip's ini loads rkbin's closed SPL
  (`rv1126b_spl_ipc_v1.06.bin`). The from-source loader puts this tree's
  own U-Boot SPL there instead, then a FIT (`u-boot.itb`, from
  `arch/arm/mach-rockchip/make_fit_atf.sh` with rkbin's BL31) at the
  offset the SPL reads.
- **Packing.** One Makefile target (a family patch) would join the IDB
  and `u-boot.itb` into `u-boot-rockchip.bin`, with the FIT at the
  sector the SPL is built to read. Buildroot then builds that name.

What the bench run confirms:

- **Serial log on UART0 (1500000 baud).** The DDR blob's banner (DDR
  type, frequency, size) proves the boot ROM took the header and ran the
  init image. The SPL's banner proves the boot image entry. U-Boot
  proper proves the FIT.
- **No output at all** means the header was refused. Compare with a card
  made by Rockchip's own tools on another machine: dump sectors 64 and up
  of both cards, and diff the header words (`BOOT0_PARAM` and the
  addresses).
- **In maskrom mode,** reading back the loader area over USB (on another
  machine with Rockchip's tools) shows what `boot_merger` writes for
  these ini values.

## DDR blob

The board names `rv1126b_ddr_lp4x_1332MHz_tb_v1.10.bin`, which is a
guess: the board's DRAM part is not confirmed. rkbin's own
`RV1126BMINIALL.ini` uses `rv1126b_ddr_1332MHz_v1.10.bin` instead (no
DRAM type in its name, and not a `_tb` thunder-boot build). The bench run
should start with the ini's blob, and read the DRAM type from its
banner.
