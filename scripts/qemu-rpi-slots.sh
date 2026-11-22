#!/usr/bin/env bash
# Boots a Raspberry Pi 5 / CM5 first-flash disk image (.img or .img.xz) in
# qemu-system-aarch64 -M virt, with no firmware: this script does the part
# of the Pi firmware a slot boot uses, then boots the kernel directly.
#
# Everything after the kernel is the image's own bytes: the initramfs and
# the command line from the boot partition, the boot partition itself, the
# root and data partitions. The kernel (--kernel: one with the drivers the
# virt machine needs) and the device tree (the virt machine's) are not, and
# each boot says which bytes those were (boot-N.replaced in the state
# directory, with their SHA-256).
#
#   scripts/qemu-rpi-slots.sh helios-...-qemu.img.xz
#   scripts/qemu-rpi-slots.sh --kernel Image --print board.img.xz
#
# What it emulates, per boot, from the image's first FAT partition:
# - config.txt (tryboot.txt on a tryboot), its `include` lines and the
#   [all], [cm5], [pi5], [tryboot], [boot_count&N], [boot_count>N],
#   [boot_count<N] and [boot_count=N] sections; other sections are skipped.
#   In the default profile the boot count those sections test stays at
#   --boot-count (0) on every boot and none is given to the OS; the board
#   profile counts boots as the firmware does (see below);
# - os_prefix, kernel= (default kernel_2712.img, else kernel8.img),
#   cmdline= (default cmdline.txt) and `initramfs <file> [followkernel]`;
#   device_tree= and dtoverlay= are not applied (the device tree is virt's),
#   except the board overlay's marker (below).
#   A file in a fixed-size bay is padded with zeros: the command line is
#   read up to its padding; the kernel takes an initramfs's zeros as it is;
# - console=serial0 becomes ttyAMA0, the virt machine's PL011, and other
#   console= arguments are dropped so the serial console is /dev/console;
#   nothing else is changed or added (--append adds), and each boot records
#   whether the line it booted is the image's;
# - /chosen/bootloader/tryboot in the device tree, 1 on a tryboot and 0
#   otherwise, and /chosen/bootloader/boot-mode (--boot-mode, 1: the SD or
#   eMMC), as the firmware sets them;
# - the boot source: the disk is a virtio-mmio device, and its device tree
#   node is moved to the path the command line names for this boot mode
#   (the word `<prefix>.src.rpi-boot-mode-<N>=<path>`), so a program that
#   finds its disk by that path, through the disk's of_node in sysfs, finds
#   this one. The command line itself is passed as it is. The moved node is
#   checked in the built device tree: there, a virtio-mmio node with the
#   disk's transport, or the boot fails with words;
# - /serial-number in the device tree (--serial, default 1000000052540077),
#   as a Pi's firmware sets it, so the board keeps one identity across runs;
# - the board overlay's marker: for each dtoverlay= the boot's config names
#   (<os_prefix><overlay_prefix, default overlays/><name>.dtbo), the
#   fragment that targets / and carries lemnos { lemnos,board;
#   lemnos,overlay-sha256 } gives those two properties to /lemnos in the
#   virt tree, as the firmware's applying the overlay would. Only that
#   fragment: the overlay's other fragments are for Pi hardware. With no
#   dtoverlay=, a file missing from the boot partition (the firmware drops
#   it without a word) or no marker fragment, nothing is added, so a
#   missing overlay is refused as on the board. Each boot records whether
#   the marker was emulated and from which file.
# Other dtoverlay= fragments and dtparam= are not applied: the device tree
# is the virt machine's, so Pi hardware (RP1, CSI, PIO, the fan) is absent.
#
# Restarts: QEMU runs with -no-reboot and the serial console goes to a log.
# When the kernel logs "Restarting system with command '...tryboot...'"
# (`reboot "0 tryboot"`), the next boot is a tryboot; any other restart or a
# panic restart boots config.txt again, which is what the firmware does (a
# panic forgets a pending tryboot). A watchdog that fires is a restart too,
# as on the board: QEMU's WATCHDOG event (read from a second QMP socket,
# <state>/qmp-events.sock, which QEMU waits on before it starts, and
# logged in <state>/boot-N.events) ends the boot, and the next boots
# config.txt. "Power down", or QEMU quitting without a restart in the log
# or a watchdog event, ends the loop.
#
# Network (--net):
# - tap: the host's tap qtap0 (a NetworkManager "shared" connection at
#   10.77.0.1/24 gives the guest DHCP), so the guest is on a board network;
# - netns: no root needed. A user and network namespace holds its own qtap0
#   at 10.77.0.1/24 and a dnsmasq handing the guest 10.77.0.50. Run tools
#   against the guest inside it, as printed at start:
#     nsenter -t "$(cat <state>/netns.pid)" -U -n CMD
#   (as the namespace's root, which is the caller on the host, so ssh's
#   ownership checks on ~/.ssh pass)
#   The guest has no route out of the namespace;
# - user: QEMU's user-mode network with --hostfwd forwards (default host
#   15800, 15899, 19200 and 2222 to the guest's 5800, 5899, 9200 and 22);
# - auto (default): tap when qtap0 exists, else netns.
# The guest's MAC is fixed (--mac, default 52:54:00:77:00:02), so its lease
# and name stay the same across boots. An i6300esb watchdog is attached for
# kernels that carry its driver, unless --no-watchdog leaves it out (each
# boot records which). Each boot also records "seccomp: on", "off" or
# "unknown", from the configuration the kernel embeds (CONFIG_IKCONFIG;
# unknown without it): systemd's SystemCallFilter= needs it on.
#
# Board profile (--profile board): the parts of a Raze-like board that the
# virt machine above leaves out, each from the board's own facts:
# - the disk is an eMMC (QEMU's emmc on sdhci-pci), as the CM5's is: the
#   kernel makes mmcblk0 and the hardware areas mmcblk0boot0, mmcblk0boot1
#   (4 MiB each, read-only) and mmcblk0rpmb (4 MiB, a character device),
#   all under one controller. The image is the user area. QEMU needs that
#   area's size to be a power of two, so it is the smallest one that holds
#   the image (--disk-size gives a larger one), not the board's; the extra
#   is past the image's partitions. The controller is on PCI, so the PCI
#   host's node moves to the boot source's path (as the virtio disk's does
#   in the default profile) and the eMMC sits under it;
# - /chosen/bootloader also has, as the board's firmware sets them:
#   count, the boot's number since power-on (1 on the run's first boot,
#   counting every restart; --boot-count N starts it at N + 1), which the
#   [boot_count...] sections test too; rsts, 0x1000 on the first boot (a
#   power-on) and 0x20 after a restart; and partition, the number of the
#   FAT partition booted. These are the values Atlas read on Raze #1 and #2;
# - the firmware's watchdog: when the boot's config.txt sets
#   kernel_watchdog_timeout, the firmware arms the SoC's watchdog before the
#   kernel starts, and the kernel finds it running. Here a node compatible
#   "gaia,firmware-watchdog" (timeout-sec from that setting) is put at the
#   path the image's own device tree has its power-management block at
#   (compatible brcm,bcm2712-pm or brcm,bcm2835-pm, whose watchdog cell the
#   firmware arms), with that block's address as the CPU sees it, so the
#   kernel names its device as the board's (107d200000.watchdog on a CM5).
#   The virt kernel's patched i6300esb (scripts/qemu-virt-kernel) registers
#   the watchdog on a "bcm2835-wdt" child of that device with no device
#   tree node of its own, as the BCM2712's MFD cell is, and starts it at
#   probe, marked as running. The device trees read are the boot's
#   device_tree= or else every .dtb in its os_prefix directory, which must
#   agree. Without kernel_watchdog_timeout there is no node, and the
#   i6300esb is the plain one, stopped until opened. --watchdog-scale P
#   makes that watchdog's hardware count P percent of each second asked
#   (gaia,time-scale-percent; the timeouts userspace sets and reads stay as
#   asked), for a guest that runs slower than the board under TCG: P is
#   the guest's time over the board's for the same stretch of the boot,
#   measured, and each boot records it as not like the board;
# The default profile (--profile virt) is unchanged: a virtio disk, and no
# count, rsts or partition in the device tree.
#
# The image is never written: its contents are copied (sparse) to
# <state>/disk.img once, and the guest writes that copy. --fresh copies again.
# Nothing is added to the copy unless asked: --ssh-key FILE puts FILE at
# board/authorized_keys on its boot partition, for an OS that reads it there
# (and the boot partition is then not the image's).
#
# Options:
#   --state DIR     work directory (default: ./qemu-<image name>)
#   --profile P     virt (default) or board (see above)
#   --fresh         start again from the image
#   --kernel FILE   boot this kernel instead of the image's (the board's
#                   kernel with the virt machine's drivers, for a board image)
#   --disk-size N   grow the disk copy to N (truncate syntax, e.g. 16G); with
#                   --profile board the eMMC's user area, a power of two
#   --net MODE      auto, tap, netns or user (see above)
#   --mac MAC       the guest NIC's MAC
#   --serial HEX    the board serial number in the device tree
#   --ssh-key FILE  a public key for board/authorized_keys (default: none)
#   --boot-count N  the boot count the config sections test (default 0);
#                   with --profile board, the count before the first boot
#   --boot-mode N   the firmware's boot mode (default 1)
#   --watchdog-scale P  with --profile board: the firmware watchdog's
#                   hardware counts P percent of each second (100-10000,
#                   default 100; see above)
#   --hostfwd H:G   with --net user: forward host TCP port H to guest port
#                   G (repeatable; replaces the defaults)
#   --append ARGS   add ARGS to every boot's kernel command line
#   --trial-append ARGS   add ARGS to tryboot boots' kernel command line
#                   only, to fail a trial on purpose and see it roll back
#   --trial-at N    with --trial-append: add them to the Nth tryboot of
#                   this run only (1 is the first; repeatable). Without it,
#                   every tryboot, an install's own trial among them
#   --tryboot       make the first boot a tryboot
#   --max-boots N   stop after N boots (default: no limit)
#   --no-watchdog   attach no i6300esb watchdog: a guest that feeds none
#                   is then not reset by it
#   --cpu --smp --mem   guest CPU (cortex-a76), CPUs (4), memory (4G)
#   --print         print what the first boot would use and the QEMU
#                   command, then exit (needs no QEMU)
#   --dtb           build and check the first boot's device tree, print
#                   the board overlay marker's line and the tree's path,
#                   then exit (needs QEMU, boots nothing)
#   -- ARGS         passed to QEMU
#
# A QMP socket is at <state>/qmp.sock (e.g. system_powerdown). Without a
# terminal on stdin, the serial console is the socket <state>/console.sock
# (e.g. socat - UNIX-CONNECT:<state>/console.sock), still logged. Needs mtools,
# sfdisk, xz for .xz images, dtc's fdtput, and qemu-system-aarch64 ($QEMU
# names another), dtc and python3 to move the disk's node; --net netns also
# needs unshare, ip and dnsmasq.
set -euo pipefail

usage() { sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$0"; }

args_given=("$@")
in_netns=0
profile=virt
watchdog_scale=100
# The CM5's eMMC hardware areas (Atlas, Raze #1 and #2: mmcblk0boot0 and
# mmcblk0boot1 4 MiB block devices, mmcblk0rpmb 4 MiB).
emmc_boot=$((4 << 20))
emmc_rpmb=$((4 << 20))
state=""
fresh=0
kernel_override=""
disk_size=""
hostfwds=()
net=auto
append=""
trial_append=""
trial_at=()
mac=52:54:00:77:00:02
serial=1000000052540077
ssh_key=""
boot_count=0
boot_mode=1
tryboot=0
max_boots=0
watchdog=1
cpu=cortex-a76
smp=4
mem=4G
print_only=0
dtb_only=0
qemu_extra=()
image=""

while [[ $# -gt 0 ]]; do
	case "$1" in
	--state) state="$2"; shift 2 ;;
	--profile) profile="$2"; shift 2 ;;
	--watchdog-scale) watchdog_scale="$2"; shift 2 ;;
	--fresh) fresh=1; shift ;;
	--kernel) kernel_override="$2"; shift 2 ;;
	--disk-size) disk_size="$2"; shift 2 ;;
	--hostfwd) hostfwds+=("$2"); shift 2 ;;
	--net) net="$2"; shift 2 ;;
	--append) append+="${append:+ }$2"; shift 2 ;;
	--trial-append) trial_append+="${trial_append:+ }$2"; shift 2 ;;
	--trial-at) trial_at+=("$2"); shift 2 ;;
	--mac) mac="$2"; shift 2 ;;
	--serial) serial="$2"; shift 2 ;;
	--ssh-key) ssh_key="$2"; shift 2 ;;
	--boot-count) boot_count="$2"; shift 2 ;;
	--boot-mode) boot_mode="$2"; shift 2 ;;
	--tryboot) tryboot=1; shift ;;
	--max-boots) max_boots="$2"; shift 2 ;;
	--no-watchdog) watchdog=0; shift ;;
	--cpu) cpu="$2"; shift 2 ;;
	--smp) smp="$2"; shift 2 ;;
	--mem) mem="$2"; shift 2 ;;
	--print) print_only=1; shift ;;
	--dtb) dtb_only=1; shift ;;
	--in-netns) in_netns=1; shift ;;
	-h | --help) usage; exit 0 ;;
	--) shift; qemu_extra=("$@"); break ;;
	-*) echo "unknown option $1 (see --help)" >&2; exit 2 ;;
	*)
		[[ -z $image ]] || { echo "one image only" >&2; exit 2; }
		image="$1"
		shift
		;;
	esac
done
[[ -n $image ]] || { usage >&2; exit 2; }
[[ -r $image ]] || { echo "cannot read $image" >&2; exit 2; }
[[ ${#hostfwds[@]} -gt 0 ]] || hostfwds=(15800:5800 15899:5899 19200:9200 2222:22)
if [[ $net == auto ]]; then
	net=netns
	[[ ! -e /sys/class/net/qtap0 ]] || net=tap
fi
case "$net" in
tap | netns | user) ;;
*) echo "--net is auto, tap, netns or user, not $net" >&2; exit 2 ;;
esac

for tool in mtype mcopy mmd sfdisk fdtput fdtget dtc python3; do
	command -v "$tool" > /dev/null || { echo "missing $tool (mtools, util-linux, dtc, python3)" >&2; exit 2; }
done
[[ $boot_count =~ ^[0-9]+$ && $boot_count -lt 256 ]] || { echo "--boot-count is 0 to 255" >&2; exit 2; }
for n in "${trial_at[@]}"; do
	[[ $n =~ ^[1-9][0-9]*$ ]] || { echo "--trial-at $n: a tryboot's number, 1 or more" >&2; exit 2; }
done
[[ ${#trial_at[@]} -eq 0 || -n $trial_append ]] || { echo "--trial-at needs --trial-append" >&2; exit 2; }
[[ $boot_mode =~ ^[0-9]+$ ]] || { echo "--boot-mode is a number" >&2; exit 2; }
[[ $watchdog_scale =~ ^[0-9]+$ && $watchdog_scale -ge 100 && $watchdog_scale -le 10000 ]] ||
	{ echo "--watchdog-scale is a percentage from 100 to 10000, not $watchdog_scale" >&2; exit 2; }
case "$profile" in
virt | board) ;;
*) echo "--profile is virt or board, not $profile" >&2; exit 2 ;;
esac
# Where the image starts in the disk file: after the eMMC's two boot areas
# and its RPMB area, the order QEMU's emmc keeps them in its backing file.
user_off=0
[[ $profile == virt ]] || user_off=$((2 * emmc_boot + emmc_rpmb))
qemu=${QEMU:-qemu-system-aarch64}
if [[ $print_only -eq 0 ]]; then
	command -v "$qemu" > /dev/null || { echo "missing $qemu (Fedora: qemu-system-aarch64-core)" >&2; exit 2; }
fi

name=$(basename "$image")
name=${name%.xz}
name=${name%.img}
state=${state:-./qemu-$name}
mkdir -p "$state"
state=$(cd "$state" && pwd)
disk="$state/disk.img"

# --net netns: run the rest of this script as root of a new user and network
# namespace (its files stay the caller's), with qtap0 and dnsmasq in it.
if [[ $net == netns && $print_only -eq 0 && $dtb_only -eq 0 ]]; then
	if [[ $in_netns -eq 0 ]]; then
		for tool in unshare ip dnsmasq; do
			command -v "$tool" > /dev/null || { echo "missing $tool (--net netns)" >&2; exit 2; }
		done
		for file in /etc/subuid /etc/subgid; do
			grep -q "^$(id -un):" "$file" || { echo "--net netns needs a range for $(id -un) in $file" >&2; exit 2; }
		done
		# --map-auto (the caller's /etc/subuid and /etc/subgid ranges, through
		# newuidmap) allows setgroups, which dnsmasq always calls.
		exec unshare --map-root-user --map-auto -n --kill-child -- "$0" --in-netns "${args_given[@]}"
	fi
	ip link set lo up
	ip tuntap add qtap0 mode tap
	ip addr add 10.77.0.1/24 dev qtap0
	ip link set qtap0 up
	# No lease or pid file: SELinux confines dnsmasq to its own directories,
	# and the guest's address is fixed by its MAC. It runs in the foreground
	# as this script's child, so it ends with the script.
	dnsmasq --conf-file=/dev/null --keep-in-foreground --log-facility=- \
		--interface=qtap0 --bind-interfaces --port=0 --no-resolv --user=root --group=root \
		--dhcp-range=10.77.0.50,10.77.0.99,12h --dhcp-host="$mac,10.77.0.50" \
		--leasefile-ro --pid-file &
	dnsmasq_pid=$!
	trap 'kill "$dnsmasq_pid" 2> /dev/null || true' EXIT
	echo $$ > "$state/netns.pid"
	echo "== guest network: 10.77.0.50 (host side 10.77.0.1) in a namespace; reach it with" >&2
	echo "   nsenter -t \$(cat $state/netns.pid) -U -n <command>" >&2
	echo "   (ssh there needs -F /dev/null: the host's root-owned ssh_config.d is unmapped," >&2
	echo "   e.g. nsenter -t \$(cat $state/netns.pid) -U -n ssh -F /dev/null root@10.77.0.50)" >&2
fi

new_copy=0
# A copy is laid out for its profile (the image at 0, or after the eMMC's
# hardware areas): one made for the other is not booted.
if [[ $fresh -eq 0 && -e $disk && $(cat "$state/disk.profile" 2> /dev/null || echo virt) != "$profile" ]]; then
	echo "$disk was made for --profile $(cat "$state/disk.profile" 2> /dev/null || echo virt): add --fresh to copy the image again for --profile $profile" >&2
	exit 2
fi
if [[ $fresh -eq 1 || ! -e $disk ]]; then
	new_copy=1
	rm -f "$disk" "$state/tryboot-next" "$state/disk.profile"
	if [[ $user_off -eq 0 ]]; then
		case "$image" in
		*.xz) xz -dc "$image" | cp --sparse=always /dev/stdin "$disk" ;;
		*) cp --sparse=always "$image" "$disk" ;;
		esac
	else
		# The hardware areas first (empty), then the image as the user area.
		truncate -s "$user_off" "$disk"
		case "$image" in
		*.xz) xz -dc "$image" ;;
		*) cat "$image" ;;
		esac | dd of="$disk" bs=1M seek=$((user_off >> 20)) conv=sparse,notrunc status=none
	fi
	echo "$profile" > "$state/disk.profile"
fi
if [[ $user_off -eq 0 ]]; then
	[[ -z $disk_size ]] || truncate -s ">$disk_size" "$disk"
else
	# The eMMC's user area: a power of two (QEMU's rule), at least the image.
	have=$(($(stat -c %s "$disk") - user_off))
	if [[ -n $disk_size ]]; then
		want=$(numfmt --from=iec "${disk_size%B}")
		[[ $((want & (want - 1))) -eq 0 ]] || { echo "--disk-size $disk_size: an eMMC's user area is a power of two (QEMU)" >&2; exit 2; }
		[[ $want -ge $have ]] || { echo "--disk-size $disk_size is smaller than the image ($have bytes)" >&2; exit 2; }
	else
		want=1
		while [[ $want -lt $have ]]; do want=$((want * 2)); done
	fi
	truncate -s $((user_off + want)) "$disk"
	emmc_user=$want
fi
[[ $tryboot -eq 0 ]] || : > "$state/tryboot-next"

# The byte offset in the disk file of the first FAT partition (MBR types b,
# c, e), as the firmware boots from it, and its number: "<offset> <n>".
fat_offset() {
	local table=$disk
	if [[ $user_off -ne 0 ]]; then
		table="$state/user-head.img"
		dd if="$disk" of="$table" bs=1M skip=$((user_off >> 20)) count=1 status=none
	fi
	sfdisk -d "$table" 2> /dev/null | awk -F'[=,]' -v off="$user_off" '
		/^\// { n++ }
		/^\// && $0 ~ /type= *(b|c|e)([^0-9a-f]|$)/ {
			for (i = 1; i <= NF; i++) if ($i ~ /start/) { gsub(/ /, "", $(i + 1)); print off + $(i + 1) * 512, n; exit }
		}'
}

fat_has() { mtype -i "$fat" "::$1" > /dev/null 2>&1; }

# Prints key=value lines the firmware would act on from config file $1,
# following includes and sections. $2 is 1 on a tryboot. An include is
# textual: a section it opens goes on after it, so cfg_on is global.
cfg_on=1
read_config() {
	local file=$1 line key value
	fat_has "$file" || { echo "the boot partition has no $file" >&2; return 1; }
	while IFS= read -r line || [[ -n $line ]]; do
		line=${line%%#*}
		line=${line%$'\r'}
		line=$(printf '%s' "$line" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
		[[ -n $line ]] || continue
		if [[ $line == \[*\] ]]; then
			case "$line" in
			"[all]" | "[cm5]" | "[pi5]") cfg_on=1 ;;
			"[tryboot]") cfg_on=$2 ;;
			"[boot_count&"*"]")
				local mask=${line#\[boot_count&}
				mask=${mask%\]}
				cfg_on=0
				[[ $mask =~ ^[0-9]+$ && $((count & mask)) -ne 0 ]] && cfg_on=1
				;;
			"[boot_count>"*"]" | "[boot_count<"*"]" | "[boot_count="*"]")
				local op=${line:11:1} n=${line:12}
				n=${n%\]}
				cfg_on=0
				if [[ $n =~ ^[0-9]+$ ]]; then
					case "$op" in
					">") [[ $count -gt $n ]] && cfg_on=1 ;;
					"<") [[ $count -lt $n ]] && cfg_on=1 ;;
					"=") [[ $count -eq $n ]] && cfg_on=1 ;;
					esac
				fi
				;;
			*) cfg_on=0 ;;
			esac
			continue
		fi
		[[ $cfg_on -eq 1 ]] || continue
		case "$line" in
		include\ *) read_config "${line#include }" "$2" ;;
		initramfs\ *) echo "initramfs=${line#initramfs }" ;;
		*=*)
			key=${line%%=*}
			value=${line#*=}
			echo "$key=$value"
			;;
		esac
	done < <(mtype -i "$fat" "::$file")
}

# Sets boot_kernel, boot_initramfs, boot_cmdline from the FAT for tryboot $1.
plan_boot() {
	local config=config.txt settings key value prefix="" kernel="" cmdline=cmdline.txt initramfs="" arg out
	local overlay_prefix=overlays/ name overlays=() device_tree=""
	boot_watchdog=""
	[[ $1 -eq 0 ]] || config=tryboot.txt
	cfg_on=1
	settings=$(read_config "$config" "$1")
	while IFS= read -r line; do
		[[ -n $line ]] || continue
		key=${line%%=*}
		value=${line#*=}
		case "$key" in
		os_prefix) prefix=$value ;;
		kernel) kernel=$value ;;
		cmdline) cmdline=$value ;;
		initramfs) initramfs=${value%% *} ;;
		overlay_prefix) overlay_prefix=$value ;;
		kernel_watchdog_timeout) boot_watchdog=$value ;;
		device_tree) device_tree=$value ;;
		dtoverlay)
			name=${value%%[,:]*}
			[[ -z $name ]] || overlays+=("$name")
			;;
		esac
	done <<< "$settings"
	boot_overlays=()
	for name in ${overlays[@]+"${overlays[@]}"}; do
		boot_overlays+=("$prefix$overlay_prefix$name.dtbo")
	done
	boot_config=$config
	boot_prefix=$prefix
	boot_device_tree=$device_tree
	if [[ -z $kernel ]]; then
		kernel=kernel8.img
		fat_has "${prefix}kernel_2712.img" && kernel=kernel_2712.img
	fi
	for f in "$kernel" "$cmdline" ${initramfs:+"$initramfs"}; do
		fat_has "$prefix$f" || { echo "$config names $prefix$f, which the boot partition lacks" >&2; return 1; }
	done
	boot_dir="$state/boot"
	rm -rf "${boot_dir:?}"
	mkdir -p "$boot_dir"
	mcopy -n -i "$fat" "::$prefix$kernel" "$boot_dir/kernel"
	boot_kernel="$boot_dir/kernel"
	boot_initramfs=""
	if [[ -n $initramfs ]]; then
		mcopy -n -i "$fat" "::$prefix$initramfs" "$boot_dir/initramfs"
		boot_initramfs="$boot_dir/initramfs"
	fi
	out=""
	image_cmdline=""
	boot_from=""
	# A command line in a fixed-size bay: its text, then zeros.
	for arg in $(mtype -i "$fat" "::$prefix$cmdline" | tr -d '\r\000'); do
		case "$arg" in
		*.src.rpi-boot-mode-"$boot_mode"=/*) boot_from=${arg#*=} ;;
		esac
		image_cmdline+="${image_cmdline:+ }$arg"
		case "$arg" in
		console=serial0,* | console=serial0) arg="console=ttyAMA0${arg#console=serial0}" ;;
		console=*) continue ;;
		esac
		out+="${out:+ }$arg"
	done
	boot_cmdline=$out
	boot_source="$config: ${prefix}$kernel${initramfs:+, $prefix$initramfs}, $prefix$cmdline"
}

# The machine. With "probe" as $1 it is for a QEMU that starts no guest
# (dumpdtb, info qtree): its network is user mode with no forwards, so a
# probe binds no host port and two runs can probe at once. The guest's
# hardware, and so its device tree, is the same either way.
machine_args() {
	local fwd netdev=tap,id=net0,ifname=qtap0,script=no,downscript=no
	if [[ ${1:-} == probe ]]; then
		netdev=user,id=net0
	elif [[ $net == user ]]; then
		netdev=user,id=net0
		for fwd in "${hostfwds[@]}"; do
			netdev+=",hostfwd=tcp::${fwd%%:*}-:${fwd##*:}"
		done
	fi
	printf '%s\n' -M virt,gic-version=3 -cpu "$cpu" -smp "$smp" -m "$mem" \
		-accel tcg,thread=multi -nographic -no-reboot \
		-drive "if=none,id=disk0,format=raw,file=$disk,discard=unmap"
	if [[ $profile == board ]]; then
		printf '%s\n' -device sdhci-pci \
			-device "emmc,drive=disk0,boot-partition-size=$emmc_boot,rpmb-partition-size=$emmc_rpmb"
	else
		printf '%s\n' -device virtio-blk-device,drive=disk0
	fi
	printf '%s\n' -netdev "$netdev" -device "virtio-net-pci,netdev=net0,mac=$mac" \
		-device virtio-rng-pci
	[[ $watchdog -eq 0 ]] || printf '%s\n' -device i6300esb
}

# The virt-mmio transport the disk is on: its base address, from QEMU's own
# device tree of buses.
disk_transport() {
	local args
	mapfile -t args < <(machine_args probe)
	printf 'info qtree\nquit\n' |
		"$qemu" "${args[@]}" -S -display none -serial none -monitor stdio 2> /dev/null |
		awk '/^ *mmio [0-9a-f]+\// { split($2, a, "/"); base = a[1] }
			/dev: virtio-blk-device/ { sub(/^0+/, "", base); print base; exit }'
}

# The board overlay's marker in the virt tree, from the dtoverlay= files
# plan_boot found (the last marker wins, as the overlays apply in order).
# Sets marker_note to what was done, for the boot's record.
emulate_marker() {
	local file dtbo="$state/overlay.dtbo" fragment board="" digest="" from="" b d why=()
	for file in ${boot_overlays[@]+"${boot_overlays[@]}"}; do
		if ! fat_has "$file"; then
			why+=("$file is not on the boot partition")
			continue
		fi
		mcopy -n -i "$fat" "::$file" "$dtbo"
		local found=0
		for fragment in $(fdtget -l "$dtbo" / 2> /dev/null); do
			[[ $(fdtget -t s "$dtbo" "/$fragment" target-path 2> /dev/null) == / ]] || continue
			b=$(fdtget -t s "$dtbo" "/$fragment/__overlay__/lemnos" lemnos,board 2> /dev/null) || continue
			d=$(fdtget -t s "$dtbo" "/$fragment/__overlay__/lemnos" lemnos,overlay-sha256 2> /dev/null) || continue
			board=$b digest=$d from="$file /$fragment" found=1
		done
		[[ $found -eq 1 ]] || why+=("$file has no marker fragment")
		rm -f "$dtbo"
	done
	if [[ -n $from ]]; then
		fdtput -p -t s "$state/virt.dtb" /lemnos lemnos,board "$board"
		fdtput -t s "$state/virt.dtb" /lemnos lemnos,overlay-sha256 "$digest"
		marker_note="board overlay marker: emulated from $from (lemnos,board \"$board\", lemnos,overlay-sha256 $digest)"
	elif [[ ${#boot_overlays[@]} -eq 0 ]]; then
		marker_note="board overlay marker: none, $boot_config names no dtoverlay="
	else
		marker_note="board overlay marker: none, $(IFS=';'; echo "${why[*]}" | sed 's/;/; /g')"
	fi
}

# The firmware's watchdog in the image's own device trees: "<path> <address>
# <size>", the power-management block's node path and its address and size
# as the CPU sees them (through every parent's ranges). Empty when none has
# one; fails when they disagree.
image_watchdog() {
	local files=() file dir
	rm -rf "$state/image-dt"
	mkdir -p "$state/image-dt"
	if [[ -n $boot_device_tree ]]; then
		files=("$boot_prefix$boot_device_tree")
	else
		dir=${boot_prefix%/}
		while read -r file; do
			files+=("${dir:+$dir/}$file")
		done < <(mdir -b -i "$fat" "::${dir:-/}" 2> /dev/null | sed 's#.*/##' | grep -i '\.dtb$' || true)
	fi
	local n=0 got=()
	for file in ${files[@]+"${files[@]}"}; do
		fat_has "$file" || continue
		n=$((n + 1))
		mcopy -n -i "$fat" "::$file" "$state/image-dt/$n.dtb"
		got+=("$state/image-dt/$n.dtb")
	done
	[[ ${#got[@]} -gt 0 ]] || return 0
	python3 - "${got[@]}" <<'FDT'
import struct, sys

def nodes(blob):
    """(path, props) for every node of a flattened device tree."""
    magic, _, off_struct, off_strings = struct.unpack(">IIII", blob[:16])
    if magic != 0xD00DFEED:
        sys.exit("not a device tree")
    pos, stack, out = off_struct, [], []
    def name_at(o):
        return blob[off_strings + o : blob.index(b"\0", off_strings + o)].decode()
    while True:
        (token,) = struct.unpack(">I", blob[pos : pos + 4])
        pos += 4
        if token == 1:
            end = blob.index(b"\0", pos)
            name = blob[pos:end].decode()
            pos = (end + 4) & ~3
            stack.append((name, {}))
            out.append(("/" + "/".join(n for n, _ in stack[1:]), stack[-1][1]))
        elif token == 2:
            stack.pop()
        elif token == 3:
            size, nameoff = struct.unpack(">II", blob[pos : pos + 8])
            pos += 8
            stack[-1][1][name_at(nameoff)] = blob[pos : pos + size]
            pos = (pos + size + 3) & ~3
        elif token == 4:
            continue
        elif token == 9:
            return out

def cells(data, n):
    return [struct.unpack(">" + "I" * n, data[i : i + 4 * n]) for i in range(0, len(data), 4 * n)]

def number(cs):
    v = 0
    for c in cs:
        v = (v << 32) | c
    return v

COMPATIBLE = {"brcm,bcm2712-pm", "brcm,bcm2835-pm"}
found = set()
for file in sys.argv[1:]:
    tree = dict(nodes(open(file, "rb").read()))
    def prop_int(path, name, default):
        data = tree.get(path, {}).get(name)
        return struct.unpack(">I", data)[0] if data else default
    for path, props in tree.items():
        compatible = set(props.get("compatible", b"").decode(errors="replace").split("\0"))
        if not compatible & COMPATIBLE or "reg" not in props:
            continue
        parent = path.rsplit("/", 1)[0] or "/"
        ac, sc = prop_int(parent, "#address-cells", 2), prop_int(parent, "#size-cells", 1)
        reg = cells(props["reg"], ac + sc)[0]
        address, size = number(reg[:ac]), number(reg[ac:])
        # Through each parent's ranges up to the root.
        node = parent
        while node != "/":
            up = node.rsplit("/", 1)[0] or "/"
            if "ranges" not in tree[node]:
                sys.exit(f"{path}: {node} has no ranges: no CPU address")
            rng = tree[node]["ranges"]
            if rng:
                cac = prop_int(node, "#address-cells", 2)
                pac = prop_int(up, "#address-cells", 2)
                csc = prop_int(node, "#size-cells", 1)
                for r in cells(rng, cac + pac + csc):
                    child, parent_at, span = number(r[:cac]), number(r[cac : cac + pac]), number(r[cac + pac :])
                    if child <= address < child + span:
                        address = address - child + parent_at
                        break
                else:
                    sys.exit(f"{path}: {address:#x} is outside {node}'s ranges")
            node = up
        found.add((path, address, size))
if len(found) > 1:
    sys.exit("the image's device trees disagree on the power-management block: " + "; ".join(f"{p} {a:#x}" for p, a, _ in sorted(found)))
for path, address, size in found:
    print(path, f"{address:#x}", f"{size:#x}")
FDT
}

# The firmware's watchdog node (see the board profile): at the image's path,
# its parents made simple buses where the virt tree has none.
firmware_watchdog() {
	local path address size part at=""
	read -r path address size <<< "$1"
	for part in $(tr '/' ' ' <<< "${path%/*}"); do
		at+="/$part"
		if ! fdtget "$state/virt.dtb" "$at" compatible > /dev/null 2>&1; then
			fdtput -p -t s "$state/virt.dtb" "$at" compatible simple-bus
			fdtput -t x "$state/virt.dtb" "$at" '#address-cells' 2
			fdtput -t x "$state/virt.dtb" "$at" '#size-cells' 2
			fdtput -t x "$state/virt.dtb" "$at" ranges
		fi
	done
	fdtput -p -t s "$state/virt.dtb" "$path" compatible gaia,firmware-watchdog
	fdtput -t x "$state/virt.dtb" "$path" reg $(printf '%x ' $((address >> 32)) $((address & 0xffffffff)) $((size >> 32)) $((size & 0xffffffff)))
	fdtput -t x "$state/virt.dtb" "$path" timeout-sec "$(printf %x "$boot_watchdog")"
	[[ $watchdog_scale -eq 100 ]] || fdtput -t x "$state/virt.dtb" "$path" gaia,time-scale-percent "$(printf %x "$watchdog_scale")"
}

# The virt machine's device tree with /chosen/bootloader/tryboot set to $1,
# the boot mode $2, the board's serial number, and the disk's node at the
# boot source's path $3 when there is one.
make_dtb() {
	local args base
	mapfile -t args < <(machine_args probe)
	"$qemu" "${args[@]}" -machine "dumpdtb=$state/virt.dtb" > /dev/null
	fdtput -p -t x "$state/virt.dtb" /chosen/bootloader tryboot "$1"
	fdtput -t x "$state/virt.dtb" /chosen/bootloader boot-mode "$(printf %x "$2")"
	fdtput -t s "$state/virt.dtb" / serial-number "$serial"
	if [[ $profile == board ]]; then
		fdtput -t x "$state/virt.dtb" /chosen/bootloader count "$(printf %x "$count")"
		fdtput -t x "$state/virt.dtb" /chosen/bootloader rsts "$(printf %x "$rsts")"
		fdtput -t x "$state/virt.dtb" /chosen/bootloader partition "$(printf %x "$fat_part")"
	fi
	emulate_marker
	move_node "$3" || return 1
	if [[ $profile == board ]]; then
		firmware_wdt=""
		if [[ -n $boot_watchdog ]]; then
			[[ $boot_watchdog =~ ^[0-9]+$ && $boot_watchdog -gt 0 ]] || { echo "$boot_config: kernel_watchdog_timeout=$boot_watchdog is not a number of seconds" >&2; return 1; }
			firmware_wdt=$(image_watchdog) || return 1
			[[ -n $firmware_wdt ]] || { echo "$boot_config sets kernel_watchdog_timeout, and the image's device trees have no power-management block (brcm,bcm2712-pm or brcm,bcm2835-pm) for the firmware's watchdog" >&2; return 1; }
			firmware_watchdog "$firmware_wdt"
		fi
	fi
}

# Moves the disk's node (the PCI host's in the board profile) to the boot
# source's path $1, and checks it there.
move_node() {
	[[ -n $1 ]] || return 0
	local node base
	if [[ $profile == board ]]; then
		# The eMMC's controller is on PCI: the PCI host's node moves.
		node=$(fdtget -l "$state/virt.dtb" / | grep -m1 '^pcie@' || true)
		[[ -n $node ]] || { echo "the virt device tree has no PCI host node" >&2; return 1; }
	else
		base=$(disk_transport)
		[[ -n $base ]] || { echo "QEMU did not say which virtio-mmio transport the disk is on" >&2; return 1; }
		node="virtio_mmio@$base"
	fi
	dtc -q -I dtb -O dts -o "$state/virt.dts" "$state/virt.dtb"
	python3 - "$state/virt.dts" "$node" "$1" <<'MOVE'
import sys
path, node, to = sys.argv[1], sys.argv[2], sys.argv[3]
lines = open(path).read().split("\n")
start = next(i for i, l in enumerate(lines) if l.strip() == node + " {")
indent = lines[start][: len(lines[start]) - len(lines[start].lstrip())]
end = next(i for i in range(start + 1, len(lines)) if lines[i] == indent + "};")
body = [l.strip() for l in lines[start + 1 : end]]
del lines[start : end + 1]
parts = [p for p in to.split("/") if p]
text = []
for depth, part in enumerate(parts):
    pad = "\t" * (depth + 1)
    text.append(pad + part + " {")
    if depth < len(parts) - 1:
        text += [pad + "\t" + p for p in ("compatible = \"simple-bus\";", "#address-cells = <0x02>;", "#size-cells = <0x02>;", "ranges;")]
    else:
        text += [pad + "\t" + p for p in body]
for depth in reversed(range(len(parts))):
    text.append("\t" * (depth + 1) + "};")
root_end = max(i for i, l in enumerate(lines) if l == "};")
lines[root_end:root_end] = text
open(path, "w").write("\n".join(lines))
MOVE
	dtc -q -I dts -O dtb -o "$state/virt.dtb" "$state/virt.dts"
	# Where the guest will look for it: there, and the disk's.
	local compatible reg
	compatible=$(fdtget "$state/virt.dtb" "$1" compatible 2> /dev/null || true)
	if [[ $profile == board ]]; then
		[[ $compatible == pci-host-ecam-generic ]] && return 0
		echo "the device tree has no PCI host node (the eMMC's controller) at $1: compatible '$compatible'" >&2
		return 1
	fi
	reg=$(fdtget -t x "$state/virt.dtb" "$1" reg 2> /dev/null || true)
	if [[ $compatible != "virtio,mmio" || " $reg " != *" $base "* ]]; then
		echo "the device tree has no virtio-mmio node for the disk (transport $base) at $1: compatible '$compatible', reg '$reg'" >&2
		return 1
	fi
}

read -r fat_at fat_part < <(fat_offset) || true
[[ -n ${fat_at:-} ]] || { echo "$image has no FAT boot partition" >&2; exit 1; }
fat="$disk@@$fat_at"

if [[ $new_copy -eq 1 && -n $ssh_key && $ssh_key != none ]]; then
	[[ -r $ssh_key ]] || { echo "cannot read --ssh-key $ssh_key" >&2; exit 2; }
	fat_has board || mmd -i "$fat" ::board
	mcopy -o -i "$fat" "$ssh_key" ::board/authorized_keys
	echo "== root's SSH key on the guest: $ssh_key (board/authorized_keys)" >&2
fi

count=$boot_count
boots=0
trials=0
# Whether the trial arguments go on this tryboot (the $trials-th of the run).
trial_faulted() {
	[[ ${#trial_at[@]} -eq 0 ]] && return 0
	local n
	for n in "${trial_at[@]}"; do [[ $n -eq $trials ]] && return 0; done
	return 1
}
# Whether seccomp with filters is on in the kernel $1, from the
# configuration it embeds (CONFIG_IKCONFIG: the gzipped .config between
# IKCFG_ST and IKCFG_ED): on, off, or unknown when it embeds none. A
# gzipped kernel is read through gzip.
kernel_seccomp() {
	local image=$1 read=(cat) offset config
	[[ $(od -An -tx1 -N2 "$image" | tr -d ' \n') != 1f8b ]] || read=(gzip -dc)
	offset=$("${read[@]}" < "$image" 2>/dev/null | grep -abo IKCFG_ST | head -n1 | cut -d: -f1 || true)
	if [[ -z $offset ]]; then
		echo unknown
		return
	fi
	# gzip stops at the end of the embedded stream and warns of the bytes after.
	config=$("${read[@]}" < "$image" 2>/dev/null | tail -c +$((offset + 9)) | gzip -dc 2>/dev/null || true)
	if [[ -z $config ]]; then
		echo unknown
	elif grep -qx 'CONFIG_SECCOMP=y' <<<"$config" && grep -qx 'CONFIG_SECCOMP_FILTER=y' <<<"$config"; then
		echo on
	else
		echo off
	fi
}
while :; do
	if [[ $profile == board ]]; then
		# The firmware's count of boots since power-on, and the reset status:
		# a power-on first, a restart after.
		count=$((boot_count + boots + 1))
		rsts=0x20
		[[ $boots -gt 0 ]] || rsts=0x1000
	fi
	this_tryboot=0
	if [[ -e $state/tryboot-next ]]; then
		this_tryboot=1
		rm -f "$state/tryboot-next"
	fi
	plan_boot "$this_tryboot"
	boot_cmdline+="${append:+ $append}"
	[[ $this_tryboot -eq 0 ]] || trials=$((trials + 1))
	if [[ $this_tryboot -eq 1 && -n $trial_append ]] && trial_faulted; then
		boot_cmdline+=" $trial_append"
	fi
	[[ -z $kernel_override ]] || boot_kernel=$kernel_override
	mapfile -t args < <(machine_args)
	args+=(-kernel "$boot_kernel" -append "$boot_cmdline")
	[[ -z $boot_initramfs ]] || args+=(-initrd "$boot_initramfs")
	args+=(-qmp "unix:$state/qmp.sock,server=on,wait=off")
	if [[ $print_only -eq 1 ]]; then
		echo "tryboot: $this_tryboot"
		echo "boot count: $count"
		echo "boot mode: $boot_mode"
		echo "profile: $profile"
		if [[ $profile == board ]]; then
			echo "disk: emmc, user area $emmc_user bytes, boot0 and boot1 $emmc_boot bytes, rpmb $emmc_rpmb bytes"
			echo "rsts: $rsts"
			echo "partition: $fat_part"
			echo "kernel_watchdog_timeout: ${boot_watchdog:-none}"
		fi
		echo "boot source: ${boot_from:-none}"
		echo "from: $boot_source"
		echo "kernel: $boot_kernel"
		echo "initramfs: ${boot_initramfs:-none}"
		echo "watchdog: $([[ $watchdog -eq 1 ]] && echo i6300esb || echo "none (--no-watchdog)")"
		echo "seccomp: $(kernel_seccomp "$boot_kernel")"
		echo "cmdline: $boot_cmdline"
		echo "overlays: ${boot_overlays[*]:-none}"
		printf 'qemu:'
		printf ' %q' "$qemu" "${args[@]}" -dtb "$state/virt.dtb" "${qemu_extra[@]}"
		echo
		exit 0
	fi
	make_dtb "$this_tryboot" "$boot_mode" "$boot_from"
	if [[ $dtb_only -eq 1 ]]; then
		echo "$marker_note"
		echo "$state/virt.dtb"
		exit 0
	fi
	boots=$((boots + 1))
	# What this boot takes that is not the image's.
	{
		whose="the image's"
		[[ -z $kernel_override ]] || whose="--kernel; not the image's"
		echo "kernel $boot_kernel sha256 $(sha256sum < "$boot_kernel" | cut -d' ' -f1) ($whose)"
		echo "device tree $state/virt.dtb sha256 $(sha256sum < "$state/virt.dtb" | cut -d' ' -f1) (the virt machine's; not the image's)"
		echo "$marker_note"
		if [[ $profile == board ]]; then
			echo "disk: QEMU emmc on sdhci-pci, user area $emmc_user bytes (a power of two; not the board's size), boot0/boot1 $emmc_boot bytes, rpmb $emmc_rpmb bytes; its PCI host at the boot source's path ${boot_from:-(none named)}"
			echo "firmware: count $count, rsts $rsts, partition $fat_part, boot-mode $boot_mode, tryboot $this_tryboot"
			if [[ -n $firmware_wdt ]]; then
				echo "firmware watchdog: armed, $boot_watchdog s (kernel_watchdog_timeout in $boot_config), node gaia,firmware-watchdog at ${firmware_wdt%% *} (the image's power-management block, CPU address ${firmware_wdt#* })"
				[[ $watchdog_scale -eq 100 ]] || echo "firmware watchdog: NOT like the board: its hardware counts $watchdog_scale% of each second asked (--watchdog-scale), for a guest slower than the board"
			else
				echo "firmware watchdog: none ($boot_config sets no kernel_watchdog_timeout)"
			fi
		fi
		if [[ $watchdog -eq 1 ]]; then
			echo "watchdog: i6300esb attached"
		else
			echo "watchdog: NONE, left out (--no-watchdog); a board has one"
		fi
		echo "seccomp: $(kernel_seccomp "$boot_kernel")"
		[[ -z $boot_initramfs ]] || echo "initramfs from the image sha256 $(sha256sum < "$boot_initramfs" | cut -d' ' -f1)"
		if [[ $boot_cmdline == "$image_cmdline" ]]; then
			echo "command line: the image's"
		else
			echo "command line: NOT the image's: booted '$boot_cmdline', the image's '$image_cmdline'"
		fi
	} > "$state/boot-$boots.replaced"
	log="$state/boot-$boots.log"
	echo "== boot $boots (tryboot $this_tryboot) from $boot_source; serial log $log" >&2
	echo "== $marker_note" >&2
	if [[ -t 0 ]]; then
		args+=(-chardev "stdio,id=con,mux=on,signal=on,logfile=$log" -serial chardev:con -mon chardev=con)
	else
		args+=(-chardev "socket,id=con,path=$state/console.sock,server=on,wait=off,logfile=$log"
			-serial chardev:con -monitor none)
	fi
	# QEMU's events, from before the first instruction: QEMU waits for this
	# listener on its socket.
	events="$state/boot-$boots.events"
	rm -f "$state/qmp-events.sock"
	python3 - "$state/qmp-events.sock" "$events" <<'QMP' &
import json, socket, sys, time
path, out = sys.argv[1], sys.argv[2]
for _ in range(600):
    try:
        s = socket.socket(socket.AF_UNIX)
        s.connect(path)
        break
    except OSError:
        s.close()
        time.sleep(0.1)
else:
    sys.exit(f"no QMP events socket at {path}")
f = s.makefile("rw")
f.readline()
f.write(json.dumps({"execute": "qmp_capabilities"}) + "\n")
f.flush()
with open(out, "w") as log:
    # QEMU starts the guest once this listener is connected.
    log.write(f"{time.time():.6f} START {{}}\n")
    log.flush()
    for line in f:
        msg = json.loads(line)
        if "event" in msg:
            stamp = msg.get("timestamp", {})
            log.write(f"{stamp.get('seconds', 0)}.{stamp.get('microseconds', 0):06d} {msg['event']} {json.dumps(msg.get('data', {}))}\n")
            log.flush()
QMP
	events_pid=$!
	status=0
	"$qemu" "${args[@]}" -qmp "unix:$state/qmp-events.sock,server=on,wait=on" -dtb "$state/virt.dtb" "${qemu_extra[@]}" || status=$?
	wait "$events_pid" || true
	if [[ $status -ne 0 ]]; then
		echo "== QEMU exited with $status" >&2
		exit "$status"
	fi
	if grep -q ' WATCHDOG ' "$events" 2> /dev/null; then
		# The watchdog's reset: config.txt again (a pending tryboot is
		# forgotten, as after a panic).
		rm -f "$state/tryboot-next"
		echo "== restart by the watchdog, $(awk '$2 == "START" { t = $1 } $2 == "WATCHDOG" && t { printf "%.1f s after QEMU started the guest", $1 - t; exit }' "$events"): $(grep -m1 ' WATCHDOG ' "$events")" >&2
	elif grep -aq "Restarting system with command '[^']*tryboot" "$log"; then
		: > "$state/tryboot-next"
		echo "== restart into tryboot" >&2
	elif grep -aqE 'reboot: Restarting system|Rebooting in [0-9]+ seconds' "$log"; then
		echo "== restart" >&2
	else
		echo "== stopped" >&2
		exit 0
	fi
	if [[ $max_boots -gt 0 && $boots -ge $max_boots ]]; then
		echo "== stopping after $boots boots (--max-boots)" >&2
		exit 0
	fi
done
