# Board-like QEMU runs (Raze first)

`scripts/qemu-rpi-slots.sh --profile board` boots a built Raze disk image under QEMU with what the virt machine
otherwise leaves out of the board. Run it on an image before it goes to a board. These runs are their own tier, not
part of the landing gate (`scripts/gate.sh`): one command per image.

## One command per image

The virt kernel, once per image kernel (the image's own kernel source and configuration, plus
`scripts/qemu-virt-kernel.config` and the patches in `scripts/qemu-virt-kernel/`; Gaia builds it, cached in the store):

```
GAIA_STORE=<store> scripts/qemu-virt-kernel.sh <devices> products/raze-base/product.toml boards/raze/board.toml <out> \
    --recipes <gaia>/recipes --setting raze-base.version=<v> --setting raze-base.slot=<slot>
```

The stock host, once per host kernel (a static x86_64 busybox is the one input not on the host; Gaia used
busybox.net's 1.35.0 x86_64-linux-musl binary):

```
scripts/qemu-stock-host.sh --busybox <busybox> <host dir>
```

Then each image:

```
scripts/qemu-rpi-slots.sh --profile board --fresh --net user --max-boots 1 \
    --kernel <out>/Image --watchdog-scale 459 --usb-host <host dir> --state <work dir> <image>.disk
```

- The image is read, never written: it is copied (sparse) into `<work dir>/disk.img`.
- The guest's serial log is `<work dir>/boot-N.log`.
- What each boot takes that is not the image's goes in `<work dir>/boot-N.replaced`: the kernel, the device tree, the
  disk shape, the firmware's values and `kernel_watchdog_timeout`.
- `--print` shows the first boot's plan and QEMU line without booting. `--dtb` builds and checks its device tree.
- `--max-boots 1` stops at the first restart.
- The kernel must have PCI host generic, SDHCI PCI, MMC block, erofs, dm-verity, ext4, vfat and i6300esb; for the
  firmware's watchdog, the i6300esb patch. `qemu-virt-kernel.sh` makes such a kernel from the board's own.
- A watchdog reset ends the boot and the next starts from config.txt, as on the board. The runner reads QEMU's
  `WATCHDOG` event from `<work dir>/qmp-events.sock` into `boot-N.events` and says how long after the guest started
  it came.
- With `--usb-host`, the stock host's serial log is `<work dir>/boot-N.usb-host.log`, and its `stock-host:` lines are
  printed when the boot ends. When the stock host is done, it powers off, the board guest is quit and the run ends.

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
- **The firmware's watchdog.** When the boot's config.txt sets `kernel_watchdog_timeout`, the board's firmware arms the
  SoC watchdog before the kernel starts. The runner puts a `gaia,firmware-watchdog` node (with `timeout-sec`) at the
  path of the image's own power-management block (`brcm,bcm2712-pm`; `/soc@107c000000/watchdog@7d200000` on the
  Raze), with its CPU address, so the kernel names the device `107d200000.watchdog` as on the board. The patched
  i6300esb registers the watchdog on a `bcm2835-wdt` child of it that has no device-tree node of its own (the BCM2712's
  MFD cell), and starts it at probe marked as running. So `/sys/class/watchdog/watchdog0/device` has no `of_node`,
  and its parent's is the image's watchdog path: the shape lemnosd's board file has to match.

- **The USB device controller.** The image's device tree has the SoC's dwc2 (`/axi/usb@480000` on the Raze,
  `brcm,bcm2835-usb`), disabled; the board overlay's fragment@7 enables it with `dr_mode = "peripheral"`. Where the
  base tree or one of the boot's `dtoverlay=` files enables it for a device (by `target-path` or by a label), the
  runner puts a `gaia,virtual-udc` node at that path, with its CPU address. The virt kernel's usbip-vudc (patch
  0002) binds to it, so the UDC is `1000480000.usb` in `/sys/class/udc`, as on the board, and lemnosd's gadget binds
  to it by that name. Without such an overlay there is no UDC, as on the board.
- **The stock host** (`--usb-host`). This is a second guest, x86_64 under KVM, that stands for a PC with nothing set
  up for the board. It runs the host's own distribution kernel and its own vhci_hcd, cdc_ncm and cdc_acm modules. The
  UDC links to it over virtio vsock as soon as the gadget pulls up, relayed by `socat` on the host. The stock host
  then does what a host does with a device plugged in:
  - prints the descriptors it read, and the interfaces and endpoints of the configuration it set;
  - leases an address on the CDC NCM interface with a plain DHCP client;
  - checks that the lease carries no host configuration: no router, name server, domain, host name, NTP server,
    WPAD or static routes;
  - pings the server the lease names. Nothing about the board's addresses is written into it.

  Its kernel log is the result.

A work dir's disk copy is laid out for one profile. Switching profiles needs `--fresh`.

## Endpoint numbering: usbip-vudc as the board's dwc2

A gadget function's endpoint addresses come from the UDC: `usb_ep_autoconfig` takes the first free endpoint on the
UDC's list that fits, and the address is the number in its name. So the same functions get the same addresses on two
UDCs only if both list the same endpoints in the same order.

- **dwc2 on the board:** `EPs: 8` (Atlas, Raze #2), ep0 and 7 general-purpose endpoints per direction. They are
  listed ep1in, ep1out, ep2in, ep2out, … ep7in, ep7out, each of any transfer type.
- **usbip-vudc as upstream has it:** ep0 and 15 per direction, in the same order (ep1in, ep1out, …), each of any type.
- **As patched (0002):** the same, capped at the node's `gaia,endpoints` (the runner gives 7, the board's), so the
  list is the board's.

The first functions take ep1in, ep1out, ep2in, ep2out, … in bind order, on either UDC. Where they still differ:
- vudc has no FIFOs: it takes any max packet size, where dwc2 sizes each IN endpoint's FIFO;
- vudc serves high speed only, over USB/IP, with no PHY;
- ep0's max packet is 64 on both.
A fault that depends on FIFO sizing would not show.

## Not like the board

- QEMU needs the eMMC's user area to be a power of two. It is the smallest one that holds the image (16 GiB for a Raze
  image), not the module's 29.1 G. `--disk-size` sets a larger one.
- The card's CID name is "QEMU!!". Timing and wear are not modelled.
- The kernel and device tree are not the image's. Each boot records which were used. The kernel is built from the
  image's own source and configuration, with the same release string, so the image's modules (lemnos.ko among them)
  load.
- The watchdog is QEMU's i6300esb behind the board's shape; its identity is "i6300ESB timer", not "Broadcom BCM2835
  Watchdog timer". The device tree node's compatible is `gaia,firmware-watchdog`, not `brcm,bcm2712-pm`.
- **Speed.** TCG runs this boot about 4.6 times slower than the board. The image's timeouts (15 s for slot-init and the
  firmware) would then expire before lemnosd starts, whatever it does. `--watchdog-scale P` makes the watchdog's
  hardware count P% of each second asked; the timeouts that userspace sets and reads stay as asked. P is measured, not
  chosen: the guest's time over the board's for the same stretch of the boot. Every boot records it as not like the
  board. A fault that is a race on time, not one about who owns the watchdog, may still not show.

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

## Proof: the reset (2026-11-22)

The cause, from Lemnos and the coordinator: lemnosd matched its board file's `watchdog:<path>` against the watchdog
device's own `of_node`. The BCM2712's watchdog is an MFD cell with none, so lemnosd never held or fed it, and the
firmware-armed watchdog reset the board about 17 s after kernel start: slot-init's last feed at about 2 s, plus 15 s.

Kernel: built by `qemu-virt-kernel.sh` from Atlas 4411fdc6's devices (Raze, Linux 7.3.5-v8, LLVM 23.1.5), 527 s cold
for the kernel step. Both images below carry the same kernel8.img as 4411fdc6.

Scale: 459%, the guest's 21.537 s over the board's 4.695 s from kernel start to lemnos.ko loading. The guest figure is
from a 47db1d24 boot; the board figure is Atlas's Raze #2 journal of 47db1d24.

| Image | Result |
|---|---|
| 0a94343e (Lemnos cb436d2a) | slot-init leaves the watchdog running at 11.3 s, and nobody feeds it: reset by the watchdog 80.3 s after the guest started, which is 17.5 s at the board's speed (the board: 16–17 s) |
| 47db1d24 (Lemnos 61c37032, the fix) | held for 229 s, three times the scaled window, until a QMP quit |

Without the scale (`--watchdog-scale 100`), 4411fdc6 resets 23.7 s after the guest started, as "Lemnos hardware
daemon" is starting. That run shows the hand-over gap, not lemnosd's match.

## Proof: the USB gadget (2026-11-22)

The cause, from Lemnos and the coordinator: lemnosd wrote its FunctionFS descriptors with `FUNCTIONFS_VIRTUAL_ADDR`.
Their virtual address 0x02 collided with NCM's real ep2out on dwc2's endpoint numbering.

Kernel: `qemu-virt-kernel.sh` on Atlas 4411fdc6's devices, with patches 0001 and 0002. Stock host: Linux
7.3.4-202.nobara.fc44.x86_64 (the build host's own), busybox 1.35.0. Scale: 459%, as above.

| Image | The board guest | The stock host |
|---|---|---|
| 0a94343e (Lemnos cb436d2a) | `ffs_func_eps_enable: config_ep_by_speed(ep1out) returned -5` | `config 1 interface 2 altsetting 1 has a duplicate endpoint with address 0x2, skipping`, then `can't set config #1, error -32`: the board's fault. Then the watchdog reset (fault A), 74.5 s after the guest started |
| 47db1d24 (Lemnos 61c37032) | gadget bound | configuration 1 set; CDC NCM `usb0` and CDC ACM `ttyACM0`; lease 172.31.131.131/29 from 172.31.131.129 with no host configuration; ping of 172.31.131.129 ok; then the run ends |

The descriptors the stock host read show the collision. On both images, autoconfig gave the FunctionFS interface
ep1in and ep1out, NCM's data interface ep2out and ep2in, and ACM's ep3out and ep4in:

| Interface | 0a94343e | 47db1d24 |
|---|---|---|
| 0, FunctionFS (class ff) | 0x81, **0x02** (the virtual address, as written) | 0x81, 0x01 |
| 1, NCM control | 0x83 | 0x83 |
| 2, NCM data | 0x02, 0x82 | 0x02, 0x82 |
| 3, ACM control | 0x85 | 0x85 |
| 4, ACM data | 0x03, 0x84 | 0x03, 0x84 |

Wall time: 85 s for 0a94343e (until the reset) and 40 s for 47db1d24 (until the stock host was done).

## Still to come

- A diff of `/sys/class/block`, `/sys/class/watchdog` and `/sys/class/udc` against listings from the real board.
