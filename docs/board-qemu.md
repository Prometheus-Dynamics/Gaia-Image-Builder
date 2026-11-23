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
  board profile, the patches in `scripts/qemu-virt-kernel/` (the watchdog, the UDC, virtio_blk's major, the
  firmware mailbox).
  `qemu-virt-kernel.sh` makes such a kernel from the board's own.
- A watchdog reset ends the boot and the next starts from config.txt, as on the board. The runner reads QEMU's
  `WATCHDOG` event from `<work dir>/qmp-events.sock` into `boot-N.events` and says how long after the guest started
  it came.
- With `--usb-host`, the stock host's serial log is `<work dir>/boot-N.usb-host.log`, and its `stock-host:` lines are
  printed when the boot ends. When the stock host is done, it powers off, the board guest is quit and the run ends.
  The vsock relay's log is `<work dir>/boot-N.usb-relay.log`: one `relay N: listening` and `relay N: ended` pair for
  each time the UDC and the stock host were joined, with socat's own lines. Keep it with the host's log: it is what
  shows a link that went away and whether it came back. The last pair of every boot ends with a reset and
  `socat exit 1`, because a guest went away; the log's closing line says so. A pair that ends that way while both
  guests are still running is the one to look at.
- A TERM or INT to the runner ends the boot's QEMU and everything it started with it (the event reader, the stock
  host, the relay and the forward) and exits 143, saying so; nothing is left running. A caller that bounds a run
  sends TERM at its limit.

A RAM boot (the EEPROM booting `boot.img` over USB or the network, as for a flasher): the image is a directory with
the EEPROM's `config.txt` (it must say `boot_ramdisk=1`) and `boot.img`:

```
scripts/qemu-rpi-slots.sh --profile board --fresh --net user --max-boots 1 \
    --kernel <out>/Image --watchdog-scale 459 --emmc-at /soc@107c000000/mmc@fff000 \
    --usb-host <host dir> --usb-host-wait 300 --usb-host-connect 9200 --state <work dir> <set dir>
```

The whole flash, with the client on the PC (`127.0.0.1:19201` is the flasher's 9200), and the reboot into the slot:

```
scripts/qemu-rpi-slots.sh --profile board --fresh --net user --max-boots 3 \
    --kernel <out>/Image --watchdog-scale 459 --serial <board serial> --emmc-at /soc@107c000000/mmc@fff000 \
    --usb-host <host dir> --usb-host-forward 19201:9200 --state <work dir> <set dir>
```

- The kernel's place, the initramfs, the command line, the device trees and the overlays come from `boot.img`'s own
  `config.txt` and `tryboot.txt`, as from a boot partition. `boot.img` is copied into `<work dir>` and only read.
- The boot mode is 3 (USB device boot) unless `--boot-mode` says. `/chosen/bootloader` has no `partition`.
- The eMMC is there and blank (a sparse 32 GiB user area, or `--disk-size`), for the image to write; nothing boots from
  it. `--emmc-at` puts its controller at the board's path for it, since a RAM boot's command line names no boot source.
- The firmware watchdog, the UDC and the stock host are as for a disk. `--usb-host-wait S` is how long the stock host
  waits for the gadget to link, and then for its network (600 s each by default); with no gadget by then it says
  `no device on vsock port`. `--usb-host-connect PORT` has it open a TCP connection to PORT on the lease's server after
  the ping, and say what came back.
- `--ssh-key` is refused: it writes a boot partition.
- `--usb-host-forward H:P` lets a real client on the PC reach the flasher. The PC's `127.0.0.1:H` goes, over vsock,
  to the stock host, which joins it to TCP P on the lease's server. With it, the stock host stays up until the boot
  ends instead of powering off. Each boot gets a new stock host, so the gadget is plugged in again after a restart, on
  the same PC port. Its log has a `stock-host: forward N:` line for each connection. The forward stays up while the
  gadget is away (see "A gadget that goes and comes back"): a connection then finds no route, as on a PC.
- A restart boots the eMMC as the RAM boot left it, unless the boot gave the firmware's one-shot RPIBOOT order
  (see "The firmware mailbox"); then it is the RAM boot again. This covers the guest's reboot, a tryboot reboot and
  the watchdog.
  It is the same eMMC image, now booted as a disk:
  - boot mode 1;
  - its boot partition's `config.txt` (or `tryboot.txt`);
  - the PCI host at the path the slot's command line names;
  - the count one higher (2 after a first boot) and rsts 0x20.
  It is recorded as `boot-2.*` of the same run. If the eMMC has no FAT boot partition by then, the run ends, since
  the firmware would go on to its next boot mode. So the whole flash can run in emulation: the RAM boot, the client's
  push through the forward, the flasher writing and checking the eMMC, the reboot, and the slot's first boot.
- Unlike the board, and recorded in `boot-N.replaced`:
  - The eMMC is QEMU's, on PCI. It writes at the host file's speed, and its user area is a power of two, not the
    board's size.
  - The gadget is usbip-vudc over USB/IP at high speed, not the dwc2. A disconnect is the USB/IP stream closing, not
    a line state on a cable.
  - The PC reaches the gadget's network through the stock host, not through an NCM interface of its own.
  - The kernel is the virt kernel, not `kernel8.img`.

The guest's sysfs against a board's, while the guest runs (without `--usb-host`, which ends the run early). The
image's console must let root log in without a password:

```
scripts/qemu-board-sysfs.py collect <work dir> <guest listing>
scripts/qemu-board-sysfs.py diff <guest listing> <board listing>...
```

`collect` waits for the console, logs in and lists `/sys/class/block`, `/sys/class/udc` and `/sys/class/watchdog`
in the form Atlas lists a board's. `diff` reads Atlas's listings as they are, several of one board merged, and prints
every fact as `same`, `differs`, `board only` or `guest only`.

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
  and its parent's is the image's watchdog path: the shape lemnosd's board file has to match. It also has the board
  driver's facts, from the node's `gaia,identity` and `gaia,max-hw-heartbeat-ms`: bcm2835_wdt's identity "Broadcom
  BCM2835 Watchdog timer", and no maximum timeout, with the hardware counting at most 15999 ms and the watchdog core
  feeding it for a longer one, as bcm2835_wdt's is fed.
- **Block majors.** The virt kernel's virtio_blk takes its major only when a virtio disk is probed (patch 0003). The
  board profile has none, so device-mapper gets 254 and `dm-0` is `254:0`, as on the board.

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
- **A gadget that goes and comes back.** On the board, unbinding the UDC (a driver rebind, or the gadget disabled
  and enabled again) is a soft disconnect: the PC sees the device go, and a new enumeration when the gadget pulls up
  again. The profile does the same. When the gadget unbinds, usbip-vudc closes its stream; the relay ends that pair
  and listens again for the next; the stock host (with `--usb-host-forward`, while it stays up) sees `USB disconnect`,
  attaches the device again when it links again (vhci_hcd's port 0 again; if vhci_hcd refused it, the next port, and
  it would say so), reads its descriptors, and leases again on the new NCM interface. When the gadget pulls up, the
  UDC links again at once and then each second until the relay takes it. A failed try is logged once per outage
  (`no link to the host over vsock port P (error): trying again each second`), and its return with how many tries it
  took, so a link that stays down is never silent. A gadget that does not come back within `--usb-host-wait` ends
  the stock host: `the device did not come back`.
  The guest's gadget is the board kernel's own: configfs, and FunctionFS for the functions a daemon serves. Only the
  UDC under it differs, so what the gadget core or f_fs refuses, it refuses here as on the dwc2. Nothing in the
  profile re-enables a gadget or hides a failure. What a process that keeps FunctionFS endpoint files open does to a
  re-enable has been run on it (Linux 7.3.3; the test `a_held_functionfs_function_is_described_again_only_when_let_go`,
  with `tests/relink/ffs-hold.rs` as the function's user side):
  - The gadget's UDC cleared and written again, endpoint files held throughout: it works. The same files serve the
    function again, and the stock host sees a disconnect and the device again.
  - The process with ep0 gone (a daemon that restarts) while another still holds copies of the endpoint files: the
    function stays as it was. A new process's descriptor write to ep0 is refused with `ESRCH` ("No such process":
    f_fs's answer to an ep0 write while the function is active and no setup is waiting), bound or unbound. Unmounting
    functionfs and mounting it again are both refused with `EBUSY`. While it is still bound, the gadget's other
    functions keep working.
  - When the copies are let go, the unmount, the mount and the descriptor write all work, the gadget binds, and the
    stock host enumerates the device again with the FunctionFS interface.
  - The function removed from configfs and made again while an old holder keeps its files: the unmount is refused
    (`EBUSY`), but the new function instance mounts and is described, since it is another instance.

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

## The firmware mailbox

The board's way back to the PC is the firmware's one-shot reboot order: a program asks it through the property
mailbox, `/dev/vcio`, and the next boot is RPIBOOT (USB device boot). The board profile has a stand-in for it, so a
return to RPIBOOT is shown before a board (Lemnos's `lemnos-way-back rpiboot`, the flasher's failure paths, slot-init's
loop return).

- **The node.** The virt kernel's `gaia-fw-mailbox` (`scripts/qemu-virt-kernel/0004-gaia-firmware-mailbox.patch`,
  built in) makes `/dev/vcio` as the Pi kernel's vcio does: a misc device, root:root 0600 (no mode given, as there),
  registered at device_initcall, so devtmpfs has it before any userspace. It binds only to the `gaia,firmware-mailbox`
  node the runner adds, so no other boot gets one.
- **The requests.** `IOCTL_MBOX_PROPERTY` (`_IOWR(100, 0, char *)`) with a property buffer as the Pi kernel takes it
  (the size in its first word, then tags to the end tag). It answers `0x80000000` and each tag's code
  `0x80000000 | 4` when it took the buffer, and `0x80000001` and takes nothing when a tag is unknown or `refuse` is
  set:
  - `0x0003808b`, the reboot order: recorded with its value (3 is RPIBOOT);
  - `0x0003808d`, the boot count set: recorded with its value;
  - `0x0003008d`, the boot count get: the count this boot's `/chosen/bootloader/count` shows. (Lemnos's own test
    stand-in answers 0.)
  `/sys/module/gaia_fw_mailbox/parameters` has `orders`, `order`, `asked`, `clears` and `refuse` (writable, and
  `gaia_fw_mailbox.refuse=1` on the command line), the names of Lemnos's `lemnos_test_vcio`.
- **The next boot.** With RPIBOOT taken, the next boot is the RAM-boot set again (boot mode 3, no partition, the eMMC
  as it is), recorded as the next `boot-N` of the run: `== RPIBOOT (the firmware's one-shot order from boot N)`.
  The order is one-shot: the boot after that goes to the eMMC. With no order, the eMMC as before. A disk run (no
  RAM-boot set) that gets the order stops: the firmware would wait for a USB host. An order value other than 3 is
  recorded and not acted on. The order holds across a watchdog reset as across a reboot.
- **How the order gets there: the runner's mechanism, nothing on the board.** The stand-in keeps its record in the
  last page of guest RAM, which the runner reserves (`/reserved-memory`, no-map) and names in the node's `reg`. In the
  board profile QEMU runs with `-action reboot=shutdown,shutdown=pause` instead of `-no-reboot`: at the guest's
  restart it pauses, the events listener saves the page (`pmemsave`, `<work dir>/boot-N.mailbox`) and quits QEMU, and
  the runner reads it. `boot-N.replaced` gets a `firmware mailbox:` line with what it held.
- **MODELLED from Lemnos's notes (docs/slots.md, "The count's life", and slot-init's e2e firmware model), not from the
  board:** the count is 8 bits, one higher at each boot, wrapping at 256; a set (`0x0003808d`) stores its value and
  the next boot reads it plus one; power-on starts at `--boot-count` (0). Also not from the board: what the firmware
  does with a set value other than 0, whether an order survives a watchdog reset (here it does), and the reply to an
  unknown tag. Atlas's board run sheet reads the count after a clear and rsts after a watchdog reset; their values
  correct this model.

## Not like the board

- QEMU needs the eMMC's user area to be a power of two. It is the smallest one that holds the image (16 GiB for a Raze
  image), not the module's 29.1 G. `--disk-size` sets a larger one.
- The card's CID name is "QEMU!!". Timing and wear are not modelled.
- The kernel and device tree are not the image's. Each boot records which were used. The kernel is built from the
  image's own source and configuration, with the same release string, so the image's modules (lemnos.ko among them)
  load.
- The eMMC controller is on PCI, so the block devices' paths have the PCI host and slot in them:
  `soc@107c000000/4010000000.mmc/pci0000:00/0000:00:01.0/mmc_host/…`, where the board has
  `soc@107c000000/1000fff000.mmc/mmc_host/…`. The node path (`of_node`) is the board's; the device names are not.
- The firmware mailbox is a stand-in that knows three tags, and an order reaches the next boot through a page the
  runner saves (see "The firmware mailbox"); the board's firmware keeps it itself. The boot count is modelled.
- The watchdog is QEMU's i6300esb behind the board's shape and facts. The device tree node's compatible is
  `gaia,firmware-watchdog`, not `brcm,bcm2712-pm`.
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

## Proof: sysfs against the board (2026-11-22)

Atlas's listings: Raze #1 running an Atlas install (`gaia-listing-raze1-993.txt` and `q4-raze1.clean.txt`, merged),
and Raze #2 at its staging boot (`gaia-listing-raze2-stage.txt`). The guest: 47db1d24 in the board profile, scale
459%, with patches 0001–0003.

Against Raze #1: 98 facts the same, 9 that differ, 7 the guest has and the listings do not.

| Differs | Board | Guest | Why |
|---|---|---|---|
| the 6 eMMC device links | `…/1000fff000.mmc/mmc_host/…` | `…/4010000000.mmc/pci0000:00/0000:00:01.0/mmc_host/…` | the controller is on PCI |
| `mmcblk0` size | 30535680 | 33554432 | QEMU's user area is a power of two |
| `mmcblk0p2` size | 30371840 | 77824 | the board's data partition fills the eMMC after the install; the image's is as built |
| `dm-0`, `loop0` sizes | 81992, 82656 | 36024, 36320 | Raze #1 runs a different image |

The same:
- every link's target under `/sys/devices` but the eMMC's: `udc/1000480000.usb` and
  `watchdog0` → `…/107d200000.watchdog/bcm2835-wdt/watchdog/watchdog0`;
- the watchdog's identity, timeout (15), state (active), status (0x8000) and bootstatus;
- the block devices' numbers, including `dm-0` 254:0 (253:0 before patch 0003);
- the boot areas (8192 sectors, `ro` and `force_ro` 1);
- the partitions' flags, and `mmcblk0p1`'s size;
- the eMMC card's attribute names.

Before patch 0001 gave the watchdog the board's identity, the guest's was "i6300ESB timer".

Against Raze #2's staging boot, the same links differ. Its watchdog is `inactive`, 0x0: that boot had no
`kernel_watchdog_timeout`. It lists no `dm-0`.

The faults above still show on these patches: 0a94343e on the same kernel gave `duplicate endpoint with address 0x2`,
`can't set config #1, error -32` and the watchdog reset 75.1 s after the guest started.
