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

/// A RAM boot's directory: the EEPROM's config.txt (`outer`) and boot.img, a
/// FAT image laid out as a boot partition, whose device tree has the
/// board's power-management block.
fn ram_boot(dir: &Path, outer: &str) -> PathBuf {
    let set = dir.join("set");
    std::fs::create_dir_all(&set).expect("mkdir");
    std::fs::write(set.join("config.txt"), outer).expect("write");
    let fat = set.join("boot.img");
    let fat_arg = fat.to_str().expect("utf-8");
    run(
        "mformat",
        &[
            "-C", "-i", fat_arg, "-T", "16384", "-h", "2", "-s", "16", "::",
        ],
    );
    let dts = dir.join("ram.dts");
    std::fs::write(
        &dts,
        "/dts-v1/;\n/ { #address-cells = <2>; #size-cells = <2>;\n soc@107c000000 { compatible = \"simple-bus\"; #address-cells = <1>; #size-cells = <1>; ranges = <0x0 0x10 0x0 0x80000000>; watchdog@7d200000 { compatible = \"brcm,bcm2712-pm\"; reg = <0x7d200000 0x308>; }; };\n};\n",
    )
    .expect("write");
    let dtb = dir.join("ram.dtb");
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
    let files: [(&str, &[u8]); 4] = [
        (
            "config.txt",
            b"kernel=kernel8.img\ninitramfs initramfs.cpio followkernel\nkernel_watchdog_timeout=15\n",
        ),
        ("cmdline.txt", b"rdinit=/init panic=5\n"),
        ("kernel8.img", &[1u8; 64]),
        ("initramfs.cpio", &[2u8; 64]),
    ];
    for (name, bytes) in files {
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
    }
    run(
        "mcopy",
        &[
            "-o",
            "-i",
            fat_arg,
            dtb.to_str().expect("utf-8"),
            "::board.dtb",
        ],
    );
    set
}

/// A RAM boot (the board profile, a directory as the image): boot.img is the
/// boot partition, read from a copy; boot mode 3; no partition in
/// /chosen/bootloader; a blank eMMC, its controller at --emmc-at; the
/// firmware's watchdog as for a disk. Refused: another profile, an outer
/// config that does not say boot_ramdisk=1, no boot.img, --ssh-key.
#[test]
fn a_ram_boot_boots_boot_img_with_a_blank_emmc() {
    if !tools_here(&[
        "mformat",
        "mcopy",
        "mtype",
        "mdir",
        "fdtput",
        "fdtget",
        "dtc",
        "python3",
        "qemu-system-aarch64",
    ]) {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let set = ram_boot(
        dir.path(),
        "# the EEPROM's\n  boot_ramdisk=1 # over USB\r\n",
    );
    let boot_img = std::fs::read(set.join("boot.img")).expect("boot.img");
    let emmc = "/soc@107c000000/mmc@fff000";
    let board = [
        "--profile",
        "board",
        "--fresh",
        "--disk-size",
        "16M",
        "--emmc-at",
        emmc,
    ];
    let (ok, out) = runner(dir.path(), &set, &[&board[..], &["--dtb"]].concat());
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
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )
    };
    assert_eq!(get(&[emmc, "compatible"]).1, "pci-host-ecam-generic");
    assert_eq!(get(&["-t", "x", "/chosen/bootloader", "boot-mode"]).1, "3");
    assert_eq!(get(&["-t", "x", "/chosen/bootloader", "count"]).1, "1");
    assert!(
        !get(&["/chosen/bootloader", "partition"]).0,
        "a partition on a RAM boot"
    );
    let wdt = "/soc@107c000000/watchdog@7d200000";
    assert_eq!(get(&[wdt, "compatible"]).1, "gaia,firmware-watchdog");
    assert_eq!(get(&["-t", "i", wdt, "timeout-sec"]).1, "15");

    // The eMMC: hardware areas and user area, all blank; boot.img untouched.
    let copy = std::fs::read(dir.path().join("state/disk.img")).expect("the eMMC");
    assert_eq!(copy.len(), (12 << 20) + (16 << 20));
    assert!(copy.iter().all(|b| *b == 0), "the eMMC is blank");
    assert_eq!(
        std::fs::read(set.join("boot.img")).expect("boot.img"),
        boot_img
    );
    let copy = dir.path().join("state/boot.img");
    assert_eq!(std::fs::read(&copy).expect("the copy"), boot_img);
    assert!(
        std::fs::metadata(&copy)
            .expect("the copy")
            .permissions()
            .readonly(),
        "the copy of boot.img is writable"
    );

    // The plan says what was booted, and --boot-mode still chooses.
    let (ok, out) = runner(
        dir.path(),
        &set,
        &[
            "--profile",
            "board",
            "--emmc-at",
            emmc,
            "--boot-mode",
            "4",
            "--print",
        ],
    );
    assert!(ok, "{out}");
    let sha = {
        let out = Command::new("sha256sum")
            .arg(set.join("boot.img"))
            .output()
            .expect("sha256sum");
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .next()
            .expect("a digest")
            .to_string()
    };
    for want in [
        "boot mode: 4",
        "partition: none (a RAM boot)",
        &format!("boot.img sha256 {sha}"),
        &format!("emmc at: {emmc}"),
        "from: config.txt: kernel8.img, initramfs.cpio, cmdline.txt",
        "cmdline: rdinit=/init panic=5",
    ] {
        assert!(out.contains(want), "{want}: {out}");
    }
    // Without --fresh, a disk copy is not taken for a RAM boot's eMMC.
    let disk = disk(dir.path());
    let (ok, out) = runner(dir.path(), &disk, &["--profile", "board", "--print"]);
    assert!(
        !ok && out.contains("was made for a RAM boot (--profile board): add --fresh to make it again for --profile board"),
        "{out}"
    );

    // The default eMMC is 32 GiB, sparse.
    let (ok, out) = runner(
        dir.path(),
        &set,
        &["--profile", "board", "--fresh", "--print"],
    );
    assert!(
        ok && out.contains(&format!("user area {} bytes", 32u64 << 30)),
        "{out}"
    );

    // Refused.
    for (outer, args, want) in [
        (
            "boot_ramdisk=1\n",
            &["--print"][..],
            "a RAM boot, which needs --profile board",
        ),
        (
            "boot_ramdisk=0\n",
            &["--profile", "board", "--print"][..],
            "does not say boot_ramdisk=1",
        ),
        (
            "#boot_ramdisk=1\n",
            &["--profile", "board", "--print"][..],
            "does not say boot_ramdisk=1",
        ),
        (
            "boot_ramdisk=1\n",
            &["--profile", "board", "--ssh-key", "key", "--print"][..],
            "a RAM boot's boot.img is only read",
        ),
        (
            "boot_ramdisk=1\n",
            &["--profile", "board", "--emmc-at", "soc/mmc", "--print"][..],
            "--emmc-at is a device-tree path",
        ),
    ] {
        std::fs::write(set.join("config.txt"), outer).expect("write");
        let (ok, out) = runner(dir.path(), &set, args);
        assert!(!ok && out.contains(want), "{outer:?} {args:?}: {out}");
    }
    // A config written on Windows: CRLF (the CR is trailing whitespace), no comment.
    std::fs::write(set.join("config.txt"), "boot_ramdisk=1\r\n").expect("write");
    std::fs::write(set.join("boot.img"), [0u8; 4096]).expect("write");
    let (ok, out) = runner(dir.path(), &set, &["--profile", "board", "--print"]);
    assert!(
        !ok && out.contains("is not a FAT image with a config.txt"),
        "{out}"
    );
    std::fs::remove_file(set.join("boot.img")).expect("rm");
    let (ok, out) = runner(dir.path(), &set, &["--profile", "board", "--print"]);
    assert!(
        !ok && out.contains("holds config.txt and boot.img"),
        "{out}"
    );
}

/// The stock host's options: how long it waits, and the TCP port it
/// connects to, on its command line and in the boot's record; refused
/// without --usb-host, or when not numbers.
#[test]
fn the_stock_hosts_wait_and_connect_are_options() {
    if !tools_here(&[
        "mformat", "mcopy", "mtype", "sfdisk", "fdtput", "dtc", "python3",
    ]) {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let disk = disk(dir.path());
    for (args, want) in [
        (
            &["--usb-host-forward", "19200:9200", "--print"][..],
            "--usb-host-wait, --usb-host-connect and --usb-host-forward need --usb-host",
        ),
        (
            &["--usb-host-forward", "9200", "--print"][..],
            "--usb-host-forward is HOST-PORT:BOARD-PORT",
        ),
        (
            &["--usb-host-wait", "30", "--print"][..],
            "--usb-host-wait, --usb-host-connect and --usb-host-forward need --usb-host",
        ),
        (
            &["--usb-host-connect", "9200", "--print"][..],
            "--usb-host-wait, --usb-host-connect and --usb-host-forward need --usb-host",
        ),
        (
            &["--usb-host-wait", "x", "--print"][..],
            "--usb-host-wait is a number of seconds",
        ),
        (
            &["--emmc-at", "/soc@107c000000/mmc@fff000", "--print"][..],
            "--emmc-at needs --profile board",
        ),
        (
            &["--usb-host-connect", "x", "--print"][..],
            "--usb-host-connect is a TCP port",
        ),
    ] {
        let (ok, out) = runner(dir.path(), &disk, args);
        assert!(!ok && out.contains(want), "{args:?}: {out}");
    }
    // The plan, with a stand-in stock host (only --print reads it).
    let host = dir.path().join("host");
    std::fs::create_dir(&host).expect("mkdir");
    std::fs::write(host.join("kernel"), b"").expect("write");
    std::fs::write(host.join("initramfs.cpio.gz"), b"").expect("write");
    let host_arg = host.to_str().expect("utf-8");
    // A tree whose dwc2 is a device, for the UDC the stock host needs.
    put_tree(
        dir.path(),
        &disk,
        "slots/bay0/board.dtb",
        "/dts-v1/;\n/ { #address-cells = <2>; #size-cells = <2>;\n soc@107c000000 { compatible = \"simple-bus\"; #address-cells = <1>; #size-cells = <1>; ranges = <0x0 0x10 0x0 0x80000000>; watchdog@7d200000 { compatible = \"brcm,bcm2712-pm\"; reg = <0x7d200000 0x308>; }; };\n axi { compatible = \"simple-bus\"; #address-cells = <2>; #size-cells = <2>; ranges; usb@480000 { compatible = \"brcm,bcm2835-usb\"; reg = <0x10 0x480000 0x0 0x10000>; status = \"okay\"; dr_mode = \"peripheral\"; }; };\n};\n",
        false,
    );
    let (ok, out) = runner(
        dir.path(),
        &disk,
        &[
            "--profile",
            "board",
            "--fresh",
            "--usb-host",
            host_arg,
            "--print",
        ],
    );
    assert!(
        ok && out.contains("waits 600 s for the gadget to link")
            && out.contains(" gaia.wait=600")
            && !out.contains("gaia.connect-port"),
        "{out}"
    );
    let (ok, out) = runner(
        dir.path(),
        &disk,
        &[
            "--profile",
            "board",
            "--fresh",
            "--usb-host",
            host_arg,
            "--usb-host-wait",
            "30",
            "--usb-host-connect",
            "9200",
            "--usb-host-forward",
            "19201:9200",
            "--print",
        ],
    );
    assert!(
        ok && out.contains("then connects to TCP 9200 on the lease server")
            && out.contains(" gaia.wait=30 gaia.connect-port=9200 gaia.forward=20000:9200")
            && out.contains(
                "usb host: forwards the PC's 127.0.0.1:19201 to TCP 9200 on the lease server"
            ),
        "{out}"
    );
}

/// A stand-in for qemu-system-aarch64 ($QEMU) that boots nothing: a call
/// without -dtb (the runner's probes of the machine) goes to the real one; a boot records its arguments and device tree
/// as boot-N in $FAKE_DIR, takes QMP's events listener, and on its first
/// boot writes $FAKE_FLASH (when set) into its drive at $FAKE_AT, as a
/// flasher would, and logs a restart.
const FAKE_QEMU: &str = r#"#!/usr/bin/env python3
import json, os, socket, sys
args = sys.argv[1:]
if "-dtb" not in args:
    os.execvp("qemu-system-aarch64", ["qemu-system-aarch64", *args])
d = os.environ["FAKE_DIR"]
n = len([f for f in os.listdir(d) if f.endswith(".args")]) + 1
with open(f"{d}/boot-{n}.args", "w") as f:
    f.write("\n".join(args))
opt = lambda name: args[args.index(name) + 1]
with open(opt("-dtb"), "rb") as src, open(f"{d}/boot-{n}.dtb", "wb") as dst:
    dst.write(src.read())
for i, a in enumerate(args):
    if a == "-qmp" and "wait=on" in args[i + 1]:
        path = args[i + 1].split(",")[0][len("unix:"):]
        srv = socket.socket(socket.AF_UNIX)
        srv.bind(path)
        srv.listen(1)
        conn, _ = srv.accept()
        # A runner that never answers fails the boot rather than hangs it.
        conn.settimeout(120)
        f = conn.makefile("rw")
        f.write('{"QMP": {}}\n'); f.flush()
        f.readline()
        f.write('{"return": {}}\n'); f.flush()
        page = os.path.join(os.environ.get("FAKE_MAILBOX_DIR", "/nonexistent"), f"boot-{n}.bin")
        if os.path.exists(page):
            # The guest's restart, paused: the runner saves the mailbox page
            # (pmemsave) and quits.
            f.write('{"timestamp": {"seconds": 1, "microseconds": 0}, "event": "SHUTDOWN", "data": {"guest": true, "reason": "guest-reset"}}\n'); f.flush()
            for line in f:
                cmd = json.loads(line)
                if cmd["execute"] == "pmemsave":
                    with open(f"{d}/boot-{n}.pmemsave", "w") as note:
                        note.write(json.dumps(cmd["arguments"]))
                    with open(page, "rb") as src, open(cmd["arguments"]["filename"], "wb") as dst:
                        dst.write(src.read().ljust(cmd["arguments"]["size"], b"\0"))
                f.write('{"return": {}}\n'); f.flush()
                if cmd["execute"] == "quit":
                    break
        conn.close()
log = [a.split("logfile=")[1].split(",")[0] for a in args if "logfile=" in a][0]
drive = [a.split("file=")[1].split(",")[0] for a in args if a.startswith("if=none")][0]
with open(log, "w") as out:
    if n == 1:
        if os.environ.get("FAKE_FLASH"):
            with open(os.environ["FAKE_FLASH"], "rb") as src, open(drive, "r+b") as dst:
                dst.seek(int(os.environ["FAKE_AT"]))
                dst.write(src.read())
    if str(n) in os.environ.get("FAKE_REBOOTS", "1").split(","):
        out.write("reboot: Restarting system\n")
"#;

/// After a RAM boot, a restart boots the eMMC as the RAM boot left it: boot
/// mode 1, its boot partition's config, count 2 and rsts 0x20, recorded as
/// boot 2 of the run; an eMMC with no boot partition then ends the run.
#[test]
fn a_restart_after_a_ram_boot_boots_the_emmc() {
    if !tools_here(&[
        "mformat",
        "mcopy",
        "mmd",
        "mtype",
        "mdir",
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
    let set = ram_boot(dir.path(), "boot_ramdisk=1\n");
    // What the flasher writes: a slot disk, its device tree with the board's
    // power-management block.
    let flash = disk(dir.path());
    put_dtb(dir.path(), &flash, "slots/bay0/board.dtb", Some(0x7d200000));
    let fake = dir.path().join("fake-qemu");
    std::fs::write(&fake, FAKE_QEMU).expect("write");
    run("chmod", &["+x", fake.to_str().expect("utf-8")]);
    let boots = |flash: Option<&Path>, name: &str| {
        let record = dir.path().join(name);
        std::fs::create_dir(&record).expect("mkdir");
        let out = Command::new(repo().join("scripts/qemu-rpi-slots.sh"))
            .env("QEMU", &fake)
            .env("FAKE_DIR", &record)
            .env("FAKE_FLASH", flash.map(Path::as_os_str).unwrap_or_default())
            .env("FAKE_AT", (12u64 << 20).to_string())
            .arg("--state")
            .arg(dir.path().join(name).join("state"))
            .args([
                "--net",
                "user",
                "--profile",
                "board",
                "--fresh",
                "--disk-size",
                "64M",
                "--emmc-at",
                "/soc@107c000000/mmc@fff000",
            ])
            .arg(&set)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("the runner runs");
        (
            record,
            out.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    };
    let (record, ok, out) = boots(Some(&flash), "flashed");
    assert!(ok, "{out}");
    assert!(
        out.contains("== the eMMC's boot partition (partition 1), after the RAM boot"),
        "{out}"
    );
    let state = record.join("state");
    let first = std::fs::read_to_string(state.join("boot-1.replaced")).expect("boot 1");
    let second = std::fs::read_to_string(state.join("boot-2.replaced")).expect("boot 2");
    assert!(first.contains("ram boot: "), "{first}");
    assert!(
        first.contains("firmware: count 1, rsts 0x1000, partition none (a RAM boot), boot-mode 3"),
        "{first}"
    );
    assert!(!second.contains("ram boot: "), "{second}");
    assert!(
        second.contains("firmware: count 2, rsts 0x20, partition 1, boot-mode 1"),
        "{second}"
    );
    assert!(
        second.contains("disk: the eMMC as the RAM boot left it, booted from its partition 1"),
        "{second}"
    );
    assert!(!state.join("boot-3.replaced").exists(), "a third boot");
    let get = |n: u32, args: &[&str]| {
        let out = Command::new("fdtget")
            .arg(record.join(format!("boot-{n}.dtb")))
            .args(args)
            .output()
            .expect("fdtget");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    assert_eq!(get(1, &["-t", "x", "/chosen/bootloader", "boot-mode"]), "3");
    assert_eq!(get(2, &["-t", "x", "/chosen/bootloader", "boot-mode"]), "1");
    assert_eq!(get(2, &["-t", "x", "/chosen/bootloader", "partition"]), "1");
    assert_eq!(
        get(2, &["/soc@107c000000/watchdog@7d200000", "compatible"]),
        "gaia,firmware-watchdog"
    );
    // Boot 2's command line is the eMMC's slot's, boot 1's boot.img's.
    let args1 = std::fs::read_to_string(record.join("boot-1.args")).expect("args");
    let args2 = std::fs::read_to_string(record.join("boot-2.args")).expect("args");
    assert!(args1.contains("rdinit=/init panic=5"), "{args1}");
    assert!(args2.contains("os.slot=s0"), "{args2}");

    // Nothing written: no boot partition, and the run ends after boot 1.
    let (record, ok, out) = boots(None, "blank");
    assert!(
        ok && out.contains("the eMMC has no FAT boot partition after the RAM boot"),
        "{out}"
    );
    assert!(!record.join("state/boot-2.replaced").exists(), "{out}");
}

/// The mailbox stand-in's record page (qemu-virt-kernel/0004), as QEMU saves
/// it: magic, version, orders, order, clears, clear value, asked, refused.
fn mailbox_page(
    orders: u32,
    order: u32,
    clears: u32,
    clear: u32,
    asked: u32,
    refused: u32,
) -> Vec<u8> {
    [0x424d_5647, 1, orders, order, clears, clear, asked, refused]
        .iter()
        .flat_map(|w: &u32| w.to_le_bytes())
        .collect()
}

/// The firmware's one-shot reboot order, through the mailbox stand-in: QEMU
/// pauses at the guest's restart and the runner saves the record page. With
/// RPIBOOT (0x0003808b, 3) taken in boot 1, boot 2 is the RAM-boot set again
/// (boot mode 3, no partition), and boot 3, with no order, the eMMC. With no
/// order, or one the stand-in does not know, boot 2 is the eMMC; a boot count
/// set is the next boot's count less one. The node and its reserved page are
/// in every board-profile device tree, and the page is the last of RAM.
#[test]
fn the_firmware_rpiboot_order_serves_the_ram_boot_again() {
    if !tools_here(&[
        "mformat",
        "mcopy",
        "mmd",
        "mtype",
        "mdir",
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
    let set = ram_boot(dir.path(), "boot_ramdisk=1\n");
    let flash = disk(dir.path());
    put_dtb(dir.path(), &flash, "slots/bay0/board.dtb", Some(0x7d200000));
    let fake = dir.path().join("fake-qemu");
    std::fs::write(&fake, FAKE_QEMU).expect("write");
    run("chmod", &["+x", fake.to_str().expect("utf-8")]);
    let boots = |name: &str, pages: &[(u32, Vec<u8>)], reboots: &str, image: &Path| {
        let record = dir.path().join(name);
        let pages_dir = record.join("pages");
        std::fs::create_dir_all(&pages_dir).expect("mkdir");
        for (n, page) in pages {
            std::fs::write(pages_dir.join(format!("boot-{n}.bin")), page).expect("page");
        }
        let out = Command::new(repo().join("scripts/qemu-rpi-slots.sh"))
            .env("QEMU", &fake)
            .env("FAKE_DIR", &record)
            .env("FAKE_FLASH", &flash)
            .env("FAKE_AT", (12u64 << 20).to_string())
            .env("FAKE_MAILBOX_DIR", &pages_dir)
            .env("FAKE_REBOOTS", reboots)
            .arg("--state")
            .arg(record.join("state"))
            .args([
                "--net",
                "user",
                "--profile",
                "board",
                "--fresh",
                "--disk-size",
                "64M",
                "--mem",
                "2G",
                "--emmc-at",
                "/soc@107c000000/mmc@fff000",
            ])
            .arg(image)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("the runner runs");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (record, out.status.success(), text)
    };
    let get_in = |record: &Path, n: u32, prop: &str| {
        let out = Command::new("fdtget")
            .arg(record.join(format!("boot-{n}.dtb")))
            .args(["-t", "x", "/chosen/bootloader", prop])
            .output()
            .expect("fdtget");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let replaced = |record: &Path, n: u32| {
        std::fs::read_to_string(record.join(format!("state/boot-{n}.replaced"))).unwrap_or_default()
    };

    // RPIBOOT taken in boot 1: the RAM boot again, then the eMMC.
    let (record, ok, out) = boots("order", &[(1, mailbox_page(1, 3, 0, 0, 1, 0))], "1,2", &set);
    assert!(ok, "{out}");
    assert!(
        out.contains(
            "== firmware mailbox, boot 1: orders 1 order 3 clears 0 clear 0 asked 1 refused 0"
        ) && out.contains(
            "== RPIBOOT (the firmware's one-shot order from boot 1): the RAM-boot set again"
        ),
        "{out}"
    );
    let (one, two, three) = (
        replaced(&record, 1),
        replaced(&record, 2),
        replaced(&record, 3),
    );
    assert!(one.contains("firmware mailbox: orders 1 order 3"), "{one}");
    assert!(
        two.contains("ram boot: ")
            && two
                .contains("firmware: count 2, rsts 0x20, partition none (a RAM boot), boot-mode 3"),
        "{two}"
    );
    assert!(
        !three.contains("ram boot: ")
            && three.contains("firmware: count 3, rsts 0x20, partition 1, boot-mode 1"),
        "{three}"
    );
    let saved = std::fs::read_to_string(record.join("boot-1.pmemsave")).expect("pmemsave");
    assert!(
        saved.contains(&format!(
            "\"val\": {}",
            0x4000_0000u64 + (2u64 << 30) - 0x1000
        )) && saved.contains("\"size\": 4096"),
        "{saved}"
    );
    let args = std::fs::read_to_string(record.join("boot-1.args")).expect("args");
    assert!(
        args.contains("-action\nreboot=shutdown,shutdown=pause") && !args.contains("-no-reboot"),
        "{args}"
    );
    let get = |n: u32, args: &[&str]| {
        let out = Command::new("fdtget")
            .arg(record.join(format!("boot-{n}.dtb")))
            .args(args)
            .output()
            .expect("fdtget");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    for n in 1..=3 {
        assert_eq!(
            get(n, &["/gaia-firmware-mailbox@bffff000", "compatible"]),
            "gaia,firmware-mailbox"
        );
        assert_eq!(
            get(n, &["-t", "x", "/gaia-firmware-mailbox@bffff000", "reg"]),
            "0 bffff000 0 1000"
        );
        assert_eq!(
            get(
                n,
                &["-t", "x", "/reserved-memory/gaia-mailbox@bffff000", "reg"]
            ),
            "0 bffff000 0 1000"
        );
        let props = Command::new("fdtget")
            .arg(record.join(format!("boot-{n}.dtb")))
            .args(["-p", "/reserved-memory/gaia-mailbox@bffff000"])
            .output()
            .expect("fdtget");
        assert!(
            String::from_utf8_lossy(&props.stdout)
                .lines()
                .any(|l| l == "no-map")
        );
    }
    assert_eq!(get(2, &["-t", "x", "/chosen/bootloader", "boot-mode"]), "3");
    assert_eq!(get(3, &["-t", "x", "/chosen/bootloader", "boot-mode"]), "1");

    // No order, the boot count set to 0: boot 2 is the eMMC, with count 1.
    let (record, ok, out) = boots("clear", &[(1, mailbox_page(0, 0, 1, 0, 1, 0))], "1", &set);
    assert!(ok, "{out}");
    assert!(!out.contains("== RPIBOOT"), "{out}");
    let two = replaced(&record, 2);
    assert!(
        !two.contains("ram boot: ")
            && two.contains("firmware: count 1, rsts 0x20, partition 1, boot-mode 1"),
        "{two}"
    );

    // An order the stand-in does not know: recorded, not acted on.
    let (record, ok, out) = boots("unknown", &[(1, mailbox_page(1, 5, 0, 0, 1, 0))], "1", &set);
    assert!(
        ok && out
            .contains("reboot order 5 is not one the stand-in knows (3 is RPIBOOT); not acted on"),
        "{out}"
    );
    assert!(!replaced(&record, 2).contains("ram boot: "), "{out}");

    // RPIBOOT taken in an eMMC boot (boot 2): boot 3 is the RAM boot again,
    // boot mode 3 with no partition, as after boot 1.
    let (record, ok, out) = boots(
        "emmc-order",
        &[(2, mailbox_page(1, 3, 0, 0, 1, 0))],
        "1,2",
        &set,
    );
    assert!(
        ok && out.contains(
            "== RPIBOOT (the firmware's one-shot order from boot 2): the RAM-boot set again"
        ),
        "{out}"
    );
    let three = replaced(&record, 3);
    assert!(
        replaced(&record, 2).contains("partition 1, boot-mode 1")
            && three.contains("ram boot: ")
            && three.contains("partition none (a RAM boot), boot-mode 3"),
        "{three}"
    );
    assert_eq!(get_in(&record, 3, "boot-mode"), "3");

    // A record of another version, or with another magic: not read.
    for (name, magic, version) in [("magic", 0x1234_5678u32, 1u32), ("version", 0x424d_5647, 2)] {
        let mut page = mailbox_page(1, 3, 0, 0, 1, 0);
        page[..4].copy_from_slice(&magic.to_le_bytes());
        page[4..8].copy_from_slice(&version.to_le_bytes());
        let (record, ok, out) = boots(name, &[(1, page)], "1", &set);
        assert!(
            ok && !out.contains("== firmware mailbox") && !out.contains("== RPIBOOT"),
            "{name}: {out}"
        );
        assert!(
            replaced(&record, 2).contains("partition 1, boot-mode 1"),
            "{name}: {out}"
        );
    }

    // A page with no record (no stand-in ran): nothing recorded, the eMMC next.
    let (record, ok, out) = boots("none", &[(1, vec![0; 64])], "1", &set);
    assert!(ok && !out.contains("== firmware mailbox"), "{out}");
    assert!(
        !replaced(&record, 1).contains("firmware mailbox: orders"),
        "{out}"
    );
    assert!(
        replaced(&record, 2).contains("partition 1, boot-mode 1"),
        "{out}"
    );

    // A disk-mode run that gets RPIBOOT has no set to serve: it stops.
    let (record, ok, out) = boots("disk", &[(1, mailbox_page(1, 3, 0, 0, 1, 0))], "1", &flash);
    assert!(
        ok && out.contains("this run has no RAM-boot set to serve; stopping"),
        "{out}"
    );
    assert!(!record.join("state/boot-2.replaced").exists(), "{out}");
}
