//! `scripts/qemu-rpi-slots.sh`: the firmware's part of a slot boot, read
//! from a boot partition laid out in bays (an `os_prefix` per pointer file,
//! a boot count filter, a trial's own config, files padded to their bay's
//! size). Skipped where mtools or sfdisk are not installed; the device tree
//! check also needs QEMU, dtc and python3.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the repository root")
}

fn have(program: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {program}")])
        .output()
        .is_ok_and(|out| out.status.success())
}

fn tools_here(programs: &[&str]) -> bool {
    let missing: Vec<&&str> = programs.iter().filter(|p| !have(p)).collect();
    if missing.is_empty() {
        return true;
    }
    assert!(
        std::env::var_os("GAIA_REQUIRE_QEMU_RUNNER").is_none(),
        "GAIA_REQUIRE_QEMU_RUNNER is set, and {missing:?} are not installed"
    );
    println!("SKIPPED: no {missing:?} here");
    false
}

fn run(program: &str, args: &[&str]) {
    let out = Command::new(program).args(args).output().expect(program);
    assert!(
        out.status.success(),
        "{program} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The marker digest the test overlay carries.
const MARKER_SHA: &str = "2910e37d9417588e329e01f30093c82fd6f2f6db6daa05f75e6a15cfd6da8fea";

/// A board overlay as `lemnos-ctl tree` writes one: a fragment for Pi
/// hardware, then (with `marker`) the marker fragment last.
fn overlay(dir: &Path, marker: bool) -> Vec<u8> {
    let mut dts = String::from(
        "/dts-v1/;\n/plugin/;\n/ {\n\tfragment@0 {\n\t\ttarget-path = \"/axi/pcie@1000120000/rp1\";\n\t\t__overlay__ {\n\t\t\tgaia-test-rp1 = \"applied\";\n\t\t};\n\t};\n",
    );
    if marker {
        dts += &format!(
            "\tfragment@1 {{\n\t\ttarget-path = \"/\";\n\t\t__overlay__ {{\n\t\t\tlemnos {{\n\t\t\t\tlemnos,board = \"raze\";\n\t\t\t\tlemnos,overlay-sha256 = \"{MARKER_SHA}\";\n\t\t\t}};\n\t\t}};\n\t}};\n"
        );
    }
    dts += "};\n";
    let source = dir.join("overlay.dts");
    let out = dir.join("overlay.dtbo");
    std::fs::write(&source, dts).expect("write");
    run(
        "dtc",
        &[
            "-q",
            "-I",
            "dts",
            "-O",
            "dtb",
            "-o",
            out.to_str().expect("utf-8"),
            source.to_str().expect("utf-8"),
        ],
    );
    std::fs::read(out).expect("read")
}

/// A disk with a FAT boot partition laid out in three bays; each bay's
/// command line names its own slot and is padded with zeros. The board
/// overlay is bay 0's with its marker, bay 1's without one, and missing
/// from bay 2.
fn disk(dir: &Path) -> PathBuf {
    let fat = dir.join("fat.img");
    let fat_arg = fat.to_str().expect("utf-8");
    run(
        "mformat",
        &[
            "-C", "-i", fat_arg, "-T", "32768", "-h", "2", "-s", "16", "::",
        ],
    );
    let write = |name: &str, bytes: &[u8]| {
        let file = dir.join("file");
        std::fs::write(&file, bytes).expect("write");
        run(
            "mcopy",
            &[
                "-o",
                "-i",
                fat_arg,
                file.to_str().expect("utf-8"),
                &format!("::{name}"),
            ],
        );
    };
    let config = "kernel_watchdog_timeout=15\ninclude default.txt\n[boot_count&4]\ninclude previous.txt\n[all]\nkernel=kernel8.img\ncmdline=cmdline.txt\ninitramfs initramfs.cpio followkernel\ndevice_tree=board.dtb\ndtoverlay=lemnos-board\n";
    write("config.txt", config.as_bytes());
    write(
        "tryboot.txt",
        config
            .replace(
                "include default.txt\n[boot_count&4]\ninclude previous.txt\n[all]\n",
                "include trial.txt\n",
            )
            .as_bytes(),
    );
    for (file, bay) in [("default.txt", 0), ("previous.txt", 1), ("trial.txt", 2)] {
        write(
            file,
            format!("os_prefix=slots/bay{bay}/\n#crc\n").as_bytes(),
        );
    }
    run("mmd", &["-i", fat_arg, "::slots"]);
    for bay in 0..3 {
        run("mmd", &["-i", fat_arg, &format!("::slots/bay{bay}")]);
        write(&format!("slots/bay{bay}/kernel8.img"), &[1u8; 64]);
        write(&format!("slots/bay{bay}/initramfs.cpio"), &[2u8; 64]);
        let mut cmdline =
            format!("quiet os.slot=s{bay} os.src.rpi-boot-mode-1=/soc@107c000000/mmc@fff000 os.src.rpi-boot-mode-6=/axi/nvme panic=5\n")
                .into_bytes();
        cmdline.resize(512, 0);
        write(&format!("slots/bay{bay}/cmdline.txt"), &cmdline);
        if bay < 2 {
            run(
                "mmd",
                &["-i", fat_arg, &format!("::slots/bay{bay}/overlays")],
            );
            write(
                &format!("slots/bay{bay}/overlays/lemnos-board.dtbo"),
                &overlay(dir, bay == 0),
            );
        }
    }
    // The disk: an MBR, the FAT at 1 MiB, an empty partition after it.
    let disk = dir.join("disk.img");
    let file = std::fs::File::create(&disk).expect("create");
    file.set_len(48 << 20).expect("size");
    let mut sfdisk = Command::new("sfdisk")
        .args(["-q", disk.to_str().expect("utf-8")])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("sfdisk");
    std::io::Write::write_all(
        sfdisk.stdin.as_mut().expect("stdin"),
        b"label: dos\nstart=2048, size=32768, type=c\nstart=36864, type=83\n",
    )
    .expect("table");
    assert!(sfdisk.wait().expect("sfdisk").success());
    run(
        "dd",
        &[
            &format!("if={}", fat.display()),
            &format!("of={}", disk.display()),
            "bs=512",
            "seek=2048",
            "conv=notrunc",
            "status=none",
        ],
    );
    disk
}

fn runner(dir: &Path, disk: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(repo().join("scripts/qemu-rpi-slots.sh"))
        .arg("--state")
        .arg(dir.join("state"))
        .args(["--net", "user"])
        .args(args)
        .arg(disk)
        .output()
        .expect("the runner runs");
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn the_runner_boots_the_bay_its_config_names() {
    if !tools_here(&[
        "mformat", "mcopy", "mmd", "mtype", "sfdisk", "fdtput", "fdtget", "dtc", "python3",
    ]) {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let disk = disk(dir.path());

    // The default: config.txt, default.txt, its bay.
    let (ok, out) = runner(dir.path(), &disk, &["--fresh", "--print"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("from: config.txt: slots/bay0/kernel8.img, slots/bay0/initramfs.cpio, slots/bay0/cmdline.txt"),
        "{out}"
    );
    // The command line as the image has it, up to its padding, with
    // nothing added; the boot source is the one for this boot mode.
    assert!(
        out.contains("\ncmdline: quiet os.slot=s0 os.src.rpi-boot-mode-1=/soc@107c000000/mmc@fff000 os.src.rpi-boot-mode-6=/axi/nvme panic=5\n"),
        "{out}"
    );
    assert!(
        out.contains("boot source: /soc@107c000000/mmc@fff000\n"),
        "{out}"
    );
    assert!(out.contains("boot mode: 1\n"), "{out}");

    // Bit 2 of the boot count: the previous slot's bay.
    let (ok, out) = runner(dir.path(), &disk, &["--boot-count", "4", "--print"]);
    assert!(ok, "{out}");
    assert!(out.contains("slots/bay1/cmdline.txt"), "{out}");
    let (_, out) = runner(dir.path(), &disk, &["--boot-count", "3", "--print"]);
    assert!(out.contains("slots/bay0/cmdline.txt"), "{out}");

    // A tryboot: tryboot.txt, the trial's bay, no count filter.
    let (ok, out) = runner(
        dir.path(),
        &disk,
        &["--tryboot", "--boot-count", "4", "--print"],
    );
    assert!(ok, "{out}");
    assert!(
        out.contains("from: tryboot.txt: slots/bay2/kernel8.img"),
        "{out}"
    );

    // Another boot mode names another source.
    let (_, out) = runner(dir.path(), &disk, &["--boot-mode", "6", "--print"]);
    assert!(out.contains("boot source: /axi/nvme\n"), "{out}");
}

/// --trial-append fails trials on purpose: on every tryboot, or with
/// --trial-at only on the tryboots it numbers, so an install's own trial
/// can pass and a later one fail. A plain boot never takes them.
#[test]
fn trial_arguments_go_on_the_tryboots_asked() {
    if !tools_here(&[
        "mformat", "mcopy", "mmd", "mtype", "sfdisk", "fdtput", "fdtget", "dtc", "python3",
    ]) {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let disk = disk(dir.path());
    let fault = "systemd.setenv=UPDATE_UNAME_R=trial-fault";
    let cmdline = |out: &str| {
        out.lines()
            .find_map(|l| l.strip_prefix("cmdline: "))
            .unwrap_or_else(|| panic!("no cmdline in {out}"))
            .to_string()
    };
    let (ok, out) = runner(
        dir.path(),
        &disk,
        &["--fresh", "--tryboot", "--trial-append", fault, "--print"],
    );
    assert!(ok, "{out}");
    assert!(cmdline(&out).ends_with(fault), "every tryboot: {out}");
    // The first tryboot is number 1: faulted only when asked.
    let (ok, out) = runner(
        dir.path(),
        &disk,
        &[
            "--tryboot",
            "--trial-append",
            fault,
            "--trial-at",
            "1",
            "--print",
        ],
    );
    assert!(ok, "{out}");
    assert!(cmdline(&out).ends_with(fault), "--trial-at 1: {out}");
    let (ok, out) = runner(
        dir.path(),
        &disk,
        &[
            "--tryboot",
            "--trial-append",
            fault,
            "--trial-at",
            "2",
            "--print",
        ],
    );
    assert!(ok, "{out}");
    assert!(
        !cmdline(&out).contains(fault),
        "--trial-at 2, first trial: {out}"
    );
    // A plain boot never.
    let (ok, out) = runner(dir.path(), &disk, &["--trial-append", fault, "--print"]);
    assert!(ok, "{out}");
    assert!(!cmdline(&out).contains(fault), "a plain boot: {out}");
    // Refused: no number, and a number without the arguments.
    let (ok, out) = runner(
        dir.path(),
        &disk,
        &["--trial-append", fault, "--trial-at", "0", "--print"],
    );
    assert!(
        !ok && out.contains("--trial-at 0: a tryboot's number"),
        "{out}"
    );
    let (ok, out) = runner(dir.path(), &disk, &["--trial-at", "1", "--print"]);
    assert!(
        !ok && out.contains("--trial-at needs --trial-append"),
        "{out}"
    );
}

/// The disk's node is moved to the boot source's path in the device tree
/// the guest gets, and checked there.
#[test]
fn the_disks_node_is_at_the_boot_sources_path() {
    if !tools_here(&[
        "mformat",
        "mcopy",
        "mmd",
        "mtype",
        "sfdisk",
        "fdtput",
        "fdtget",
        "dtc",
        "python3",
        "qemu-system-aarch64",
    ]) {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let disk = disk(dir.path());
    let (ok, out) = runner(dir.path(), &disk, &["--fresh", "--dtb"]);
    assert!(ok, "{out}");
    let dtb = out
        .lines()
        .last()
        .expect("the device tree's path")
        .to_string();
    let get = |args: &[&str]| {
        let out = Command::new("fdtget")
            .arg(&dtb)
            .args(args)
            .output()
            .expect("fdtget");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    assert_eq!(
        get(&["/soc@107c000000/mmc@fff000", "compatible"]),
        "virtio,mmio"
    );
    assert_eq!(get(&["-t", "x", "/chosen/bootloader", "boot-mode"]), "1");
    assert_eq!(get(&["-t", "x", "/chosen/bootloader", "tryboot"]), "0");
    // No firmware counter on the virt machine: none is given.
    let out = Command::new("fdtget")
        .args([&dtb, "/chosen/bootloader", "count"])
        .output()
        .expect("fdtget");
    assert!(!out.status.success(), "a count was given");
}

/// Puts a device tree at `name` on the disk's boot partition: a SoC bus at
/// /soc@107c000000 whose ranges put its 0 at 0x10_0000_0000 (as the
/// BCM2712's does), with a power-management block at `pm` when given.
fn put_dtb(dir: &Path, disk: &Path, name: &str, pm: Option<u32>) {
    let block = match pm {
        Some(at) => format!(
            "watchdog@{at:x} {{ compatible = \"brcm,bcm2712-pm\"; reg = <{at:#x} 0x308>; }};"
        ),
        None => String::new(),
    };
    let dts = dir.join("image.dts");
    std::fs::write(
        &dts,
        format!(
            "/dts-v1/;\n/ {{ #address-cells = <2>; #size-cells = <2>;\n soc@107c000000 {{ compatible = \"simple-bus\"; #address-cells = <1>; #size-cells = <1>; ranges = <0x0 0x10 0x0 0x80000000>; {block} }};\n}};\n"
        ),
    )
    .expect("write");
    let dtb = dir.join("image.dtb");
    run(
        "dtc",
        &[
            "-q",
            "-I",
            "dts",
            "-O",
            "dtb",
            "-o",
            dtb.to_str().expect("utf-8"),
            dts.to_str().expect("utf-8"),
        ],
    );
    run(
        "mcopy",
        &[
            "-o",
            "-i",
            &format!("{}@@1M", disk.display()),
            dtb.to_str().expect("utf-8"),
            &format!("::{name}"),
        ],
    );
}

/// The board profile: the disk is an eMMC whose user area is the image,
/// after the two boot areas and the RPMB area (4 MiB each), a power of two
/// in size; the PCI host it sits on is at the boot source's path; and
/// /chosen/bootloader has the firmware's count (1 on the first boot), rsts
/// (0x1000, a power-on) and the FAT's partition number.
#[test]
fn the_board_profile_gives_an_emmc_and_the_firmwares_facts() {
    if !tools_here(&[
        "mformat",
        "mcopy",
        "mmd",
        "mtype",
        "sfdisk",
        "fdtput",
        "fdtget",
        "dtc",
        "python3",
        "qemu-system-aarch64",
    ]) {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let disk = disk(dir.path());
    put_dtb(dir.path(), &disk, "slots/bay0/board.dtb", Some(0x7d200000));
    // Another tree beside it, which the boot's device_tree= leaves unread.
    put_dtb(dir.path(), &disk, "slots/bay0/other.dtb", Some(0x7d300000));
    let (ok, out) = runner(
        dir.path(),
        &disk,
        &["--profile", "board", "--fresh", "--dtb"],
    );
    assert!(ok, "{out}");
    let dtb = out
        .lines()
        .last()
        .expect("the device tree's path")
        .to_string();
    // Kept: later runs write the state's tree again.
    let first = dir.path().join("first.dtb");
    std::fs::write(&first, std::fs::read(&dtb).expect("the tree")).expect("copy");
    let dtb = first.display().to_string();
    let get = |args: &[&str]| {
        let out = Command::new("fdtget")
            .arg(&dtb)
            .args(args)
            .output()
            .expect("fdtget");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    assert_eq!(
        get(&["/soc@107c000000/mmc@fff000", "compatible"]),
        "pci-host-ecam-generic"
    );
    // The firmware's watchdog: at the image's power-management block's path,
    // its address as the CPU sees it, armed for kernel_watchdog_timeout.
    let wdt = "/soc@107c000000/watchdog@7d200000";
    assert_eq!(get(&[wdt, "compatible"]), "gaia,firmware-watchdog");
    assert_eq!(get(&["-t", "x", wdt, "reg"]), "10 7d200000 0 308");
    assert_eq!(get(&["-t", "i", wdt, "timeout-sec"]), "15");
    // The board driver's facts: bcm2835_wdt's identity and hardware limit.
    assert_eq!(
        get(&[wdt, "gaia,identity"]),
        "Broadcom BCM2835 Watchdog timer"
    );
    assert_eq!(get(&["-t", "i", wdt, "gaia,max-hw-heartbeat-ms"]), "15999");
    assert_eq!(get(&["/soc@107c000000", "compatible"]), "simple-bus");
    let unscaled = Command::new("fdtget")
        .args([&dtb, wdt, "gaia,time-scale-percent"])
        .output()
        .expect("fdtget");
    assert!(!unscaled.status.success(), "a time scale by default");
    // --watchdog-scale: the hardware's count stretched, and said so.
    let (ok, out) = runner(
        dir.path(),
        &disk,
        &["--profile", "board", "--watchdog-scale", "250", "--dtb"],
    );
    assert!(ok, "{out}");
    let scaled = out
        .lines()
        .last()
        .expect("the device tree's path")
        .to_string();
    let percent = Command::new("fdtget")
        .args(["-t", "i", &scaled, wdt, "gaia,time-scale-percent"])
        .output()
        .expect("fdtget");
    assert_eq!(String::from_utf8_lossy(&percent.stdout).trim(), "250");
    for bad in ["99", "10001", "x"] {
        let (ok, out) = runner(
            dir.path(),
            &disk,
            &["--profile", "board", "--watchdog-scale", bad, "--dtb"],
        );
        assert!(
            !ok && out.contains("--watchdog-scale is a percentage from 100 to 10000"),
            "{bad}: {out}"
        );
    }
    // Booted from elsewhere (mode 6, /axi/nvme), the bus is made for it.
    let (ok, out) = runner(
        dir.path(),
        &disk,
        &["--profile", "board", "--boot-mode", "6", "--dtb"],
    );
    assert!(ok, "{out}");
    let other = out
        .lines()
        .last()
        .expect("the device tree's path")
        .to_string();
    let get_other = |args: &[&str]| {
        let out = Command::new("fdtget")
            .arg(&other)
            .args(args)
            .output()
            .expect("fdtget");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    assert_eq!(
        get_other(&["/axi/nvme", "compatible"]),
        "pci-host-ecam-generic"
    );
    assert_eq!(get_other(&["/soc@107c000000", "compatible"]), "simple-bus");
    assert_eq!(
        get_other(&["-t", "x", "/soc@107c000000", "#address-cells"]),
        "2"
    );
    assert_eq!(get_other(&["-t", "x", wdt, "reg"]), "10 7d200000 0 308");
    assert_eq!(get(&["-t", "x", "/chosen/bootloader", "count"]), "1");
    assert_eq!(get(&["-t", "x", "/chosen/bootloader", "rsts"]), "1000");
    assert_eq!(get(&["-t", "x", "/chosen/bootloader", "partition"]), "1");
    assert_eq!(get(&["-t", "x", "/chosen/bootloader", "boot-mode"]), "1");

    // The copy: 12 MiB of hardware areas, then the image, in a 64 MiB user
    // area (the 48 MiB image rounded up to a power of two).
    let copy = dir.path().join("state/disk.img");
    let bytes = std::fs::read(&copy).expect("the copy");
    let image = std::fs::read(&disk).expect("the image");
    assert_eq!(bytes.len(), (12 << 20) + (64 << 20));
    assert!(
        bytes[..12 << 20].iter().all(|b| *b == 0),
        "the hardware areas are empty"
    );
    assert!(
        bytes[12 << 20..(12 << 20) + image.len()] == image[..],
        "the user area is the image"
    );
    assert!(bytes[(12 << 20) + image.len()..].iter().all(|b| *b == 0));

    // The machine: QEMU's eMMC, with the board's hardware areas, on PCI.
    let (ok, out) = runner(dir.path(), &disk, &["--profile", "board", "--print"]);
    assert!(ok, "{out}");
    // The QEMU line is shell-quoted (printf %q escapes commas).
    let out = out.replace("\\,", ",");
    for want in [
        "-device sdhci-pci",
        "-device emmc,drive=disk0,boot-partition-size=4194304,rpmb-partition-size=4194304",
        "kernel_watchdog_timeout: 15",
        "rsts: 0x1000",
        "partition: 1",
        "boot count: 1",
        &format!("user area {} bytes", 64 << 20),
    ] {
        assert!(out.contains(want), "{want}: {out}");
    }
    assert!(!out.contains("virtio-blk-device"), "{out}");

    // A copy made for one profile is not booted with the other.
    let (ok, out) = runner(dir.path(), &disk, &["--print"]);
    assert!(
        !ok && out.contains("was made for --profile board: add --fresh"),
        "{out}"
    );
    let (ok, out) = runner(dir.path(), &disk, &["--fresh", "--print"]);
    assert!(ok && out.contains("virtio-blk-device"), "{out}");
    assert_eq!(
        std::fs::metadata(&copy).expect("the copy").len(),
        image.len() as u64
    );
    let (ok, out) = runner(dir.path(), &disk, &["--profile", "board", "--print"]);
    assert!(
        !ok && out.contains("was made for --profile virt: add --fresh"),
        "{out}"
    );

    // Refused: another profile, and a user area that is not a power of two
    // or does not hold the image.
    for (args, want) in [
        (
            &["--profile", "pi", "--print"][..],
            "--profile is virt or board, not pi",
        ),
        (
            &[
                "--profile",
                "board",
                "--fresh",
                "--disk-size",
                "96M",
                "--print",
            ][..],
            "is a power of two",
        ),
        (
            &[
                "--profile",
                "board",
                "--fresh",
                "--disk-size",
                "32M",
                "--print",
            ][..],
            "is smaller than the image",
        ),
    ] {
        let (ok, out) = runner(dir.path(), &disk, args);
        assert!(!ok && out.contains(want), "{args:?}: {out}");
    }
    let (ok, out) = runner(
        dir.path(),
        &disk,
        &[
            "--profile",
            "board",
            "--fresh",
            "--disk-size",
            "128M",
            "--print",
        ],
    );
    assert!(
        ok && out.contains(&format!("user area {} bytes", 128 << 20)),
        "{out}"
    );

    // A boot whose device trees have no power-management block, while its
    // config arms the firmware's watchdog, is refused; one that does not arm
    // it gets no node.
    put_dtb(dir.path(), &disk, "slots/bay0/board.dtb", None);
    let (ok, out) = runner(
        dir.path(),
        &disk,
        &["--profile", "board", "--fresh", "--dtb"],
    );
    assert!(
        !ok && out.contains("have no power-management block"),
        "{out}"
    );
    let config = dir.path().join("config.txt");
    std::fs::write(&config, "include default.txt\n[all]\nkernel=kernel8.img\ncmdline=cmdline.txt\ninitramfs initramfs.cpio followkernel\ndevice_tree=board.dtb\n").expect("write");
    run(
        "mcopy",
        &[
            "-o",
            "-i",
            &format!("{}@@1M", disk.display()),
            config.to_str().expect("utf-8"),
            "::config.txt",
        ],
    );
    let (ok, out) = runner(
        dir.path(),
        &disk,
        &["--profile", "board", "--fresh", "--dtb"],
    );
    assert!(ok, "{out}");
    let bare = out
        .lines()
        .last()
        .expect("the device tree's path")
        .to_string();
    let none = Command::new("fdtget")
        .args([&bare, wdt, "compatible"])
        .output()
        .expect("fdtget");
    assert!(
        !none.status.success(),
        "a watchdog node with no kernel_watchdog_timeout"
    );
}

/// The [boot_count...] sections: & tests bits, and >, < and = compare, in
/// both profiles. The board profile's count is the boot's number since
/// power-on, so --boot-count N gives N + 1 on its first boot.
#[test]
fn boot_count_sections_compare_as_the_firmware_does() {
    if !tools_here(&["mformat", "mcopy", "mmd", "mtype", "sfdisk"]) {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let disk = disk(dir.path());
    let slot = |out: &str| {
        out.lines()
            .find_map(|l| l.strip_prefix("cmdline: "))
            .unwrap_or_default()
            .split_whitespace()
            .find_map(|w| w.strip_prefix("os.slot="))
            .unwrap_or_default()
            .to_string()
    };
    for (section, count, virt, board) in [
        ("[boot_count&4]", "3", "s0", "s1"),
        ("[boot_count&4]", "4", "s1", "s1"),
        ("[boot_count>3]", "3", "s0", "s1"),
        ("[boot_count>3]", "2", "s0", "s0"),
        ("[boot_count<3]", "2", "s1", "s0"),
        ("[boot_count<3]", "3", "s0", "s0"),
        ("[boot_count=4]", "4", "s1", "s0"),
        ("[boot_count=4]", "3", "s0", "s1"),
        ("[boot_count>x]", "9", "s0", "s0"),
    ] {
        let config = format!(
            "kernel_watchdog_timeout=15\ninclude default.txt\n{section}\ninclude previous.txt\n[all]\nkernel=kernel8.img\ncmdline=cmdline.txt\ninitramfs initramfs.cpio followkernel\ndevice_tree=board.dtb\ndtoverlay=lemnos-board\n"
        );
        let file = dir.path().join("config.txt");
        std::fs::write(&file, config).expect("write");
        run(
            "mcopy",
            &[
                "-o",
                "-i",
                &format!("{}@@1M", disk.display()),
                file.to_str().expect("utf-8"),
                "::config.txt",
            ],
        );
        for (profile, want) in [("virt", virt), ("board", board)] {
            let (ok, out) = runner(
                dir.path(),
                &disk,
                &[
                    "--profile",
                    profile,
                    "--fresh",
                    "--boot-count",
                    count,
                    "--print",
                ],
            );
            assert!(ok, "{out}");
            assert_eq!(
                slot(&out),
                want,
                "{section} --boot-count {count} --profile {profile}: {out}"
            );
        }
    }
}

/// The board overlay's marker reaches /lemnos of the virt tree from the
/// dtoverlay= the boot's config names, and only the marker: a bay whose
/// overlay has no marker, or whose overlay file is missing, gets none, so
/// slot-init refuses it as a board would.
#[test]
fn the_board_overlays_marker_is_emulated_from_the_boots_overlay() {
    if !tools_here(&[
        "mformat",
        "mcopy",
        "mmd",
        "mtype",
        "sfdisk",
        "fdtput",
        "fdtget",
        "dtc",
        "python3",
        "qemu-system-aarch64",
    ]) {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let disk = disk(dir.path());
    let get = |dtb: &str, args: &[&str]| {
        let out = Command::new("fdtget")
            .arg(dtb)
            .args(args)
            .output()
            .expect("fdtget");
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let dtb_of = |out: &str| {
        out.lines()
            .last()
            .expect("the device tree's path")
            .to_string()
    };

    // Bay 0: the marker, and nothing of the overlay's other fragment.
    let (ok, out) = runner(dir.path(), &disk, &["--fresh", "--dtb"]);
    assert!(ok, "{out}");
    assert!(
        out.contains(
            "board overlay marker: emulated from slots/bay0/overlays/lemnos-board.dtbo /fragment@1"
        ),
        "{out}"
    );
    let dtb = dtb_of(&out);
    assert_eq!(
        get(&dtb, &["/lemnos", "lemnos,board"]).as_deref(),
        Some("raze")
    );
    assert_eq!(
        get(&dtb, &["/lemnos", "lemnos,overlay-sha256"]).as_deref(),
        Some(MARKER_SHA)
    );
    let text = Command::new("dtc")
        .args(["-q", "-I", "dtb", "-O", "dts", &dtb])
        .output()
        .expect("dtc");
    assert!(
        !String::from_utf8_lossy(&text.stdout).contains("gaia-test-rp1"),
        "a fragment other than the marker was applied"
    );

    // Bay 1 (boot count 4): an overlay with no marker fragment.
    let (ok, out) = runner(
        dir.path(),
        &disk,
        &["--fresh", "--boot-count", "4", "--dtb"],
    );
    assert!(ok, "{out}");
    assert!(
        out.contains("board overlay marker: none, slots/bay1/overlays/lemnos-board.dtbo has no marker fragment"),
        "{out}"
    );
    assert_eq!(get(&dtb_of(&out), &["/lemnos", "lemnos,board"]), None);

    // Bay 2 (the trial): the overlay file is missing.
    let (ok, out) = runner(dir.path(), &disk, &["--fresh", "--tryboot", "--dtb"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("board overlay marker: none, slots/bay2/overlays/lemnos-board.dtbo is not on the boot partition"),
        "{out}"
    );
    assert_eq!(get(&dtb_of(&out), &["/lemnos", "lemnos,board"]), None);
}

/// Compiles `dts` (with -@ for a base tree, so overlays can name its
/// labels) and puts it on the disk's FAT as `name`.
fn put_tree(dir: &Path, disk: &Path, name: &str, dts: &str, symbols: bool) {
    let source = dir.join("tree.dts");
    let out = dir.join("tree.dtb");
    std::fs::write(&source, dts).expect("write");
    let mut args = vec!["-q", "-I", "dts", "-O", "dtb"];
    if symbols {
        args.push("-@");
    }
    args.extend([
        "-o",
        out.to_str().expect("utf-8"),
        source.to_str().expect("utf-8"),
    ]);
    run("dtc", &args);
    run(
        "mcopy",
        &[
            "-o",
            "-i",
            &format!("{}@@1M", disk.display()),
            out.to_str().expect("utf-8"),
            &format!("::{name}"),
        ],
    );
}

/// The board profile's UDC: a gaia,virtual-udc node at the image's dwc2
/// path, with its CPU address and the board's 7 endpoints per direction,
/// only where the base tree or the boot's overlay enables the dwc2 for a
/// device (target-path or a label's fixup); with --usb-host, the vsock port
/// it links to and a vsock device on the machine.
#[test]
fn the_udc_is_at_the_images_dwc2_when_the_boot_enables_it() {
    if !tools_here(&[
        "mformat",
        "mcopy",
        "mmd",
        "mtype",
        "sfdisk",
        "fdtput",
        "fdtget",
        "dtc",
        "python3",
        "qemu-system-aarch64",
    ]) {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let disk = disk(dir.path());
    put_tree(
        dir.path(),
        &disk,
        "slots/bay0/board.dtb",
        "/dts-v1/;\n/ { #address-cells = <2>; #size-cells = <2>;\n soc@107c000000 { compatible = \"simple-bus\"; #address-cells = <1>; #size-cells = <1>; ranges = <0x0 0x10 0x0 0x80000000>; watchdog@7d200000 { compatible = \"brcm,bcm2712-pm\"; reg = <0x7d200000 0x308>; }; };\n axi { compatible = \"simple-bus\"; #address-cells = <2>; #size-cells = <2>; ranges; usb: usb@480000 { compatible = \"brcm,bcm2835-usb\"; reg = <0x10 0x480000 0x0 0x10000>; status = \"disabled\"; }; };\n};\n",
        true,
    );
    let host = dir.path().join("stock-host");
    std::fs::create_dir_all(&host).expect("mkdir");
    std::fs::write(host.join("kernel"), b"k").expect("write");
    std::fs::write(host.join("initramfs.cpio.gz"), b"i").expect("write");
    let host = host.to_str().expect("utf-8");
    let udc = "/axi/usb@480000";
    let overlay = |body: &str| {
        put_tree(
            dir.path(),
            &disk,
            "slots/bay0/overlays/lemnos-board.dtbo",
            &format!("/dts-v1/;\n/plugin/;\n/ {{\n fragment@0 {{ {body} }};\n}};\n"),
            false,
        );
    };
    // Each run copies the image again (--fresh): the overlay changes in it.
    let tree = |args: &[&str]| -> (bool, String, String) {
        let mut args = args.to_vec();
        args.push("--fresh");
        let (ok, out) = runner(dir.path(), &disk, &args);
        let dtb = out.lines().last().unwrap_or_default().to_string();
        (ok, out, dtb)
    };
    let get = |dtb: &str, args: &[&str]| {
        let out = Command::new("fdtget")
            .arg(dtb)
            .args(args)
            .output()
            .expect("fdtget");
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };

    // Disabled in the base tree and by no overlay: no UDC, and a stock host
    // has nothing to plug into.
    overlay("target-path = \"/axi/pcie@1000120000/rp1\"; __overlay__ { status = \"okay\"; };");
    let (ok, out, dtb) = tree(&["--profile", "board", "--dtb"]);
    assert!(ok, "{out}");
    assert_eq!(get(&dtb, &[udc, "compatible"]), None);
    let (ok, out, _) = tree(&["--profile", "board", "--usb-host", host, "--dtb"]);
    assert!(
        !ok && out.contains("no USB device controller for it")
            && out.contains("disabled: status \"disabled\" (from the base tree)"),
        "{out}"
    );

    // Enabled as a peripheral by the boot's overlay, by path.
    overlay(
        "target-path = \"/axi/usb@480000\"; __overlay__ { status = \"okay\"; dr_mode = \"peripheral\"; };",
    );
    let (ok, out, dtb) = tree(&["--profile", "board", "--dtb"]);
    assert!(ok, "{out}");
    assert_eq!(
        get(&dtb, &[udc, "compatible"]).as_deref(),
        Some("gaia,virtual-udc")
    );
    assert_eq!(
        get(&dtb, &["-t", "x", udc, "reg"]).as_deref(),
        Some("10 480000 0 10000")
    );
    assert_eq!(
        get(&dtb, &["-t", "i", udc, "gaia,endpoints"]).as_deref(),
        Some("7")
    );
    assert_eq!(
        get(&dtb, &[udc, "gaia,vsock-port"]),
        None,
        "a port with no host"
    );
    assert_eq!(
        get(&dtb, &["/axi", "compatible"]).as_deref(),
        Some("simple-bus")
    );
    let (ok, out, dtb) = tree(&["--profile", "board", "--usb-host", host, "--dtb"]);
    assert!(ok, "{out}");
    let port: u32 = get(&dtb, &["-t", "i", udc, "gaia,vsock-port"])
        .expect("the vsock port")
        .parse()
        .expect("a number");
    assert!((40000..60000).contains(&port), "{port}");
    let (ok, out, _) = tree(&["--profile", "board", "--usb-host", host, "--print"]);
    let out = out.replace("\\,", ",");
    assert!(
        ok && out.contains("-device vhost-vsock-pci,guest-cid="),
        "{out}"
    );

    // As a host only, through the base tree's label: no UDC.
    overlay("target = <&usb>; __overlay__ { status = \"okay\"; dr_mode = \"host\"; };");
    let (ok, out, _) = tree(&["--profile", "board", "--usb-host", host, "--dtb"]);
    assert!(
        !ok && out.contains("disabled: dr_mode \"host\" (status from slots/bay0/overlays/lemnos-board.dtbo /fragment@0)"),
        "{out}"
    );
    overlay("target = <&usb>; __overlay__ { status = \"okay\"; };");
    let (ok, out, dtb) = tree(&["--profile", "board", "--dtb"]);
    assert!(ok, "{out}");
    assert_eq!(
        get(&dtb, &[udc, "compatible"]).as_deref(),
        Some("gaia,virtual-udc")
    );

    // Refused: a stock host off the board profile, or with nothing to boot.
    let (ok, out, _) = tree(&["--usb-host", host, "--print"]);
    assert!(
        !ok && out.contains("--usb-host needs --profile board"),
        "{out}"
    );
    let empty = dir.path().join("empty");
    std::fs::create_dir_all(&empty).expect("mkdir");
    let (ok, out, _) = tree(&[
        "--profile",
        "board",
        "--usb-host",
        empty.to_str().expect("utf-8"),
        "--print",
    ]);
    assert!(
        !ok && out.contains("no kernel and initramfs.cpio.gz there"),
        "{out}"
    );
}

/// The watchdog is attached unless --no-watchdog leaves it out, and the
/// boot says which.
#[test]
fn no_watchdog_leaves_the_i6300esb_out() {
    if !tools_here(&["mformat", "mcopy", "mmd", "mtype", "sfdisk", "dtc"]) {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let disk = disk(dir.path());
    let (ok, out) = runner(dir.path(), &disk, &["--fresh", "--print"]);
    assert!(ok, "{out}");
    assert!(out.contains("\nwatchdog: i6300esb\n"), "{out}");
    assert!(out.contains(" -device i6300esb"), "{out}");
    let (ok, out) = runner(dir.path(), &disk, &["--fresh", "--no-watchdog", "--print"]);
    assert!(ok, "{out}");
    assert!(out.contains("\nwatchdog: none (--no-watchdog)\n"), "{out}");
    assert!(!out.contains("i6300esb"), "{out}");
}

/// A kernel image for `--kernel`: some bytes, then, when `config` is given,
/// that configuration embedded as CONFIG_IKCONFIG embeds it (gzipped
/// between IKCFG_ST and IKCFG_ED), then more bytes; the whole gzipped when
/// `gzipped`.
fn kernel_with(dir: &Path, name: &str, config: Option<&str>, gzipped: bool) -> PathBuf {
    let mut image = vec![0x55u8; 4096];
    if let Some(config) = config {
        let text = dir.join(format!("{name}.config"));
        std::fs::write(&text, config).expect("write");
        run("gzip", &["-n9", &text.display().to_string()]);
        image.extend_from_slice(b"IKCFG_ST");
        image.extend(std::fs::read(dir.join(format!("{name}.config.gz"))).expect("read"));
        image.extend_from_slice(b"IKCFG_ED");
    }
    image.extend(vec![0xaau8; 4096]);
    let path = dir.join(name);
    std::fs::write(&path, &image).expect("write");
    if gzipped {
        run("gzip", &["-n9", &path.display().to_string()]);
        std::fs::rename(dir.join(format!("{name}.gz")), &path).expect("rename");
    }
    path
}

/// The boot says whether the kernel has seccomp with filters, from the
/// configuration it embeds: on, off, or unknown when it embeds none; a
/// gzipped kernel is read through gzip.
#[test]
fn the_boot_says_whether_the_kernel_has_seccomp() {
    if !tools_here(&["mformat", "mcopy", "mmd", "mtype", "sfdisk", "dtc", "gzip"]) {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let disk = disk(dir.path());
    let on = "CONFIG_IKCONFIG=y\nCONFIG_SECCOMP=y\nCONFIG_SECCOMP_FILTER=y\n";
    let no_filter = "CONFIG_IKCONFIG=y\nCONFIG_SECCOMP=y\n# CONFIG_SECCOMP_FILTER is not set\n";
    for (name, config, gzipped, expected) in [
        ("on", Some(on), false, "on"),
        ("on-gzipped", Some(on), true, "on"),
        ("no-filter", Some(no_filter), false, "off"),
        ("none", None, false, "unknown"),
        ("none-gzipped", None, true, "unknown"),
    ] {
        let kernel = kernel_with(dir.path(), name, config, gzipped);
        let (ok, out) = runner(
            dir.path(),
            &disk,
            &[
                "--fresh",
                "--kernel",
                &kernel.display().to_string(),
                "--print",
            ],
        );
        assert!(ok, "{out}");
        assert!(
            out.contains(&format!("\nseccomp: {expected}\n")),
            "{name}: {out}"
        );
    }
    // The image's own kernel, which embeds no configuration.
    let (ok, out) = runner(dir.path(), &disk, &["--fresh", "--print"]);
    assert!(ok, "{out}");
    assert!(out.contains("\nseccomp: unknown\n"), "{out}");
}
