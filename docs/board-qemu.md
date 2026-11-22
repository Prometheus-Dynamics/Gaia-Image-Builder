# Board-like QEMU runs (Raze first)

`scripts/qemu-rpi-slots.sh --profile board` boots a built Raze disk image under QEMU with what the virt machine
otherwise leaves out of the board. Run it on an image before it goes to a board. These runs are their own tier, not
part of the landing gate (`scripts/gate.sh`): one command per image.

## One command per image

```
scripts/qemu-rpi-slots.sh --profile board --fresh --net user --max-boots 1 \
    --kernel <virt kernel Image> --state <work dir> <image>.disk
```

- The image is read, never written: it is copied (sparse) into `<work dir>/disk.img`.
- The guest's serial log is `<work dir>/boot-N.log`.
- What each boot takes that is not the image's goes in `<work dir>/boot-N.replaced`: the kernel, the device tree, the
  disk shape, the firmware's values and `kernel_watchdog_timeout`.
- `--print` shows the first boot's plan and QEMU line without booting. `--dtb` builds and checks its device tree.
- `--max-boots 1` stops at the first restart.
- The kernel must have PCI host generic, SDHCI PCI, MMC block, erofs, dm-verity, ext4, vfat and i6300esb.

## What the profile gives

- **Disk.** QEMU's `emmc` on `sdhci-pci`, as on the CM5:
  - `mmcblk0` is the image;
  - `mmcblk0boot0` and `mmcblk0boot1` are 4 MiB each, read-only;
  - `mmcblk0rpmb` is 4 MiB, a character device.
  The PCI host's device tree node moves to the path the command line's `<prefix>.src.rpi-boot-mode-N=` names, so all
  four sit under the boot source's node, as on the board.
- **Firmware values** in `/chosen/bootloader`, from Atlas's readings on Raze #1 and #2:
  - `count`, the boot's number since power-on (1 on the first boot). `[boot_count&N]`, `[boot_count>N]`,
    `[boot_count<N]` and `[boot_count=N]` test it.
  - `rsts`: 0x1000 at power-on, 0x20 after a restart.
  - `partition`: the FAT partition booted.
  - `boot-mode` and `tryboot`, as in the default profile.
- **`kernel_watchdog_timeout`** from the boot's config, recorded per boot. This runner does not arm it yet.

A work dir's disk copy is laid out for one profile. Switching profiles needs `--fresh`.

## Not like the board

- QEMU needs the eMMC's user area to be a power of two. It is the smallest one that holds the image (16 GiB for a Raze
  image), not the module's 29.1 G. `--disk-size` sets a larger one.
- The card's CID name is "QEMU!!". Timing and wear are not modelled.
- The kernel and device tree are not the image's. Each boot records which were used.

## Proof: the disk fault (2026-11-22)

Atlas's built first-flash images, one boot each. The kernel was Lemnos's BDW virt kernel 7.3.5 (sha256
`1ae22da9dc68a3b7…`), copied read-only.

| Image | Result |
|---|---|
| 89d9005d (build/out2) | `slot-init: 3 whole disks sit under /soc@107c000000/mmc@fff000 (mmcblk0, mmcblk0boot0, mmcblk0boot1): refusing to guess`, then a restart: the board's fault |
| b38a1ec9 (build3/out) | the same refusal |
| 4411fdc6 (build4/out) | `slot-init: the boot disk is mmcblk0`; the slot starts |

Each run took 40–67 s of wall time and 18–29 CPU-s.

On 4411fdc6, systemd then stopped: `Failed to mount cgroup2`, then "Freezing execution". The kernel used has no cgroup2.
That is a kernel gap, not an image fault. The next step builds the virt kernel from the image's own kernel source and
`.config`.

## Still to come

- The virt kernel from the image's own source and config. The firmware-armed watchdog (i6300esb started at probe with
  `WDOG_HW_RUNNING`), proven by the reset about 12 s after lemnosd starts.
- The USB device side: usbip-vudc as `1000480000.usb`, plus a stock-host guest. The host's kernel log is the result.
- A diff of `/sys/class/block`, `/sys/class/watchdog` and `/sys/class/udc` against listings from the real board.
