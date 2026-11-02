#!/usr/bin/env bash
# Boots a Raspberry Pi 5 / CM5 disk image (a Raze dynamic-slots first-flash
# .img or .img.xz) in qemu-system-aarch64 -M virt, with no firmware: this
# script does the part of the Pi firmware the slots use, then boots the
# kernel directly.
#
#   scripts/qemu-rpi-slots.sh helios-...-qemu.img.xz
#   scripts/qemu-rpi-slots.sh --kernel Image --print board.img.xz
#
# What it emulates, per boot, from the image's first FAT partition:
# - config.txt (tryboot.txt on a tryboot), its `include` lines and the
#   [all], [cm5], [pi5] and [tryboot] sections; other sections are skipped;
# - os_prefix, kernel= (default kernel_2712.img, else kernel8.img),
#   cmdline= (default cmdline.txt) and `initramfs <file> [followkernel]`;
# - console=serial0 becomes ttyAMA0, the virt machine's PL011, and other
#   console= arguments are dropped so the serial console is /dev/console;
#   net.ifnames=0 keeps the virtio NIC eth0, a Pi's Ethernet name, which
#   the OS's network configuration matches (enp0s2 otherwise);
# - /chosen/bootloader/tryboot in the device tree, 1 on a tryboot and 0
#   otherwise, as the firmware sets it, and atlas.tryboot=0|1 on the
#   command line for a slot-init that has no device tree to read;
# - /serial-number in the device tree (--serial, default 1000000052540077),
#   as a Pi's firmware sets it, so the board keeps one identity across runs.
# dtoverlay= and dtparam= are not applied: the device tree is the virt
# machine's, so Pi hardware (RP1, CSI, PIO, the fan) is absent.
#
# Restarts: QEMU runs with -no-reboot and the serial console goes to a log.
# When the kernel logs "Restarting system with command '...tryboot...'"
# (`reboot "0 tryboot"`), the next boot is a tryboot; any other restart or a
# panic restart boots config.txt again, which is what the firmware does (a
# panic forgets a pending tryboot). A tryboot.req file on the FAT, which
# Atlas's slots writer leaves when it sees atlas.tryboot= on its command
# line, also asks for one; the runner deletes it, as the firmware's flag is
# one-shot. "Power down", or QEMU quitting without a restart in the log,
# ends the loop.
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
# kernels that carry its driver.
#
# The image is never written: its contents are copied (sparse) to
# <state>/disk.img once, and the guest writes that copy. --fresh copies again.
#
# Options:
#   --state DIR     work directory (default: ./qemu-<image name>)
#   --fresh         start again from the image
#   --kernel FILE   boot this kernel instead of the image's (e.g. the Raze
#                   kernel with its qemu fragment, for a board image)
#   --disk-size N   grow the disk copy to N (truncate syntax, e.g. 16G)
#   --net MODE      auto, tap, netns or user (see above)
#   --mac MAC       the guest NIC's MAC
#   --serial HEX    the board serial number in the device tree
#   --hostfwd H:G   with --net user: forward host TCP port H to guest port
#                   G (repeatable; replaces the defaults)
#   --append ARGS   add ARGS to every boot's kernel command line
#   --tryboot       make the first boot a tryboot
#   --max-boots N   stop after N boots (default: no limit)
#   --cpu --smp --mem   guest CPU (cortex-a76), CPUs (4), memory (4G)
#   --print         print what the first boot would use and the QEMU
#                   command, then exit (needs no QEMU)
#   -- ARGS         passed to QEMU
#
# A QMP socket is at <state>/qmp.sock (e.g. system_powerdown). Without a
# terminal on stdin, the serial console is the socket <state>/console.sock
# (e.g. socat - UNIX-CONNECT:<state>/console.sock), still logged. Needs mtools,
# sfdisk, xz for .xz images, dtc's fdtput, and qemu-system-aarch64 ($QEMU
# names another); --net netns also needs unshare, ip and dnsmasq.
set -euo pipefail

usage() { sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$0"; }

args_given=("$@")
in_netns=0
state=""
fresh=0
kernel_override=""
disk_size=""
hostfwds=()
net=auto
append=""
mac=52:54:00:77:00:02
serial=1000000052540077
tryboot=0
max_boots=0
cpu=cortex-a76
smp=4
mem=4G
print_only=0
qemu_extra=()
image=""

while [[ $# -gt 0 ]]; do
	case "$1" in
	--state) state="$2"; shift 2 ;;
	--fresh) fresh=1; shift ;;
	--kernel) kernel_override="$2"; shift 2 ;;
	--disk-size) disk_size="$2"; shift 2 ;;
	--hostfwd) hostfwds+=("$2"); shift 2 ;;
	--net) net="$2"; shift 2 ;;
	--append) append+="${append:+ }$2"; shift 2 ;;
	--mac) mac="$2"; shift 2 ;;
	--serial) serial="$2"; shift 2 ;;
	--tryboot) tryboot=1; shift ;;
	--max-boots) max_boots="$2"; shift 2 ;;
	--cpu) cpu="$2"; shift 2 ;;
	--smp) smp="$2"; shift 2 ;;
	--mem) mem="$2"; shift 2 ;;
	--print) print_only=1; shift ;;
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

for tool in mtype mcopy mdel sfdisk fdtput; do
	command -v "$tool" > /dev/null || { echo "missing $tool (mtools, util-linux, dtc)" >&2; exit 2; }
done
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
if [[ $net == netns && $print_only -eq 0 ]]; then
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
fi

if [[ $fresh -eq 1 || ! -e $disk ]]; then
	rm -f "$disk" "$state/tryboot-next"
	case "$image" in
	*.xz) xz -dc "$image" | cp --sparse=always /dev/stdin "$disk" ;;
	*) cp --sparse=always "$image" "$disk" ;;
	esac
fi
[[ -z $disk_size ]] || truncate -s ">$disk_size" "$disk"
[[ $tryboot -eq 0 ]] || : > "$state/tryboot-next"

# The byte offset of the first FAT partition (MBR types b, c, e), as the
# firmware boots from it.
fat_offset() {
	sfdisk -d "$disk" | awk -F'[=,]' '
		/^\// && $0 ~ /type= *(b|c|e)([^0-9a-f]|$)/ {
			for (i = 1; i <= NF; i++) if ($i ~ /start/) { gsub(/ /, "", $(i + 1)); print $(i + 1) * 512; exit }
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
		esac
	done <<< "$settings"
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
	for arg in $(mtype -i "$fat" "::$prefix$cmdline" | tr -d '\r'); do
		case "$arg" in
		console=serial0,* | console=serial0) arg="console=ttyAMA0${arg#console=serial0}" ;;
		console=*) continue ;;
		esac
		out+="${out:+ }$arg"
	done
	boot_cmdline=$out
	boot_source="$config: ${prefix}$kernel${initramfs:+, $prefix$initramfs}, $prefix$cmdline"
}

machine_args() {
	local fwd netdev=tap,id=net0,ifname=qtap0,script=no,downscript=no
	if [[ $net == user ]]; then
		netdev=user,id=net0
		for fwd in "${hostfwds[@]}"; do
			netdev+=",hostfwd=tcp::${fwd%%:*}-:${fwd##*:}"
		done
	fi
	printf '%s\n' -M virt,gic-version=3 -cpu "$cpu" -smp "$smp" -m "$mem" \
		-accel tcg,thread=multi -nographic -no-reboot \
		-drive "if=none,id=disk0,format=raw,file=$disk,discard=unmap" \
		-device virtio-blk-pci,drive=disk0 \
		-netdev "$netdev" -device "virtio-net-pci,netdev=net0,mac=$mac" \
		-device virtio-rng-pci -device i6300esb
}

# The virt machine's device tree with /chosen/bootloader/tryboot set to $1
# and the board's serial number.
make_dtb() {
	local args
	mapfile -t args < <(machine_args)
	"$qemu" "${args[@]}" -machine "dumpdtb=$state/virt.dtb" > /dev/null
	fdtput -p -t x "$state/virt.dtb" /chosen/bootloader tryboot "$1"
	fdtput -t s "$state/virt.dtb" / serial-number "$serial"
}

fat="$disk@@$(fat_offset)"
[[ $fat != "$disk@@" ]] || { echo "$image has no FAT boot partition" >&2; exit 1; }

boots=0
while :; do
	this_tryboot=0
	if [[ -e $state/tryboot-next ]]; then
		this_tryboot=1
		rm -f "$state/tryboot-next"
	fi
	if fat_has tryboot.req; then
		this_tryboot=1
		[[ $print_only -eq 1 ]] || mdel -i "$fat" ::tryboot.req
	fi
	plan_boot "$this_tryboot"
	boot_cmdline+=" net.ifnames=0 atlas.tryboot=$this_tryboot${append:+ $append}"
	[[ -z $kernel_override ]] || boot_kernel=$kernel_override
	mapfile -t args < <(machine_args)
	args+=(-kernel "$boot_kernel" -append "$boot_cmdline")
	[[ -z $boot_initramfs ]] || args+=(-initrd "$boot_initramfs")
	args+=(-qmp "unix:$state/qmp.sock,server=on,wait=off")
	if [[ $print_only -eq 1 ]]; then
		echo "tryboot: $this_tryboot"
		echo "from: $boot_source"
		echo "kernel: $boot_kernel"
		echo "initramfs: ${boot_initramfs:-none}"
		echo "cmdline: $boot_cmdline"
		printf 'qemu:'
		printf ' %q' "$qemu" "${args[@]}" -dtb "$state/virt.dtb" "${qemu_extra[@]}"
		echo
		exit 0
	fi
	make_dtb "$this_tryboot"
	boots=$((boots + 1))
	log="$state/boot-$boots.log"
	echo "== boot $boots (tryboot $this_tryboot) from $boot_source; serial log $log" >&2
	if [[ -t 0 ]]; then
		args+=(-chardev "stdio,id=con,mux=on,signal=on,logfile=$log" -serial chardev:con -mon chardev=con)
	else
		args+=(-chardev "socket,id=con,path=$state/console.sock,server=on,wait=off,logfile=$log"
			-serial chardev:con -monitor none)
	fi
	status=0
	"$qemu" "${args[@]}" -dtb "$state/virt.dtb" "${qemu_extra[@]}" || status=$?
	if [[ $status -ne 0 ]]; then
		echo "== QEMU exited with $status" >&2
		exit "$status"
	fi
	if grep -aq "Restarting system with command '[^']*tryboot" "$log"; then
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
