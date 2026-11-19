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

/// A disk with a FAT boot partition laid out in three bays; each bay's
/// command line names its own slot and is padded with zeros.
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
