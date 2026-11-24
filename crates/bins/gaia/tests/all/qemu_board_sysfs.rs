//! scripts/qemu-board-sysfs.py: the board profile's sysfs listing against a
//! board's. `diff` reads Atlas's listing forms and the guest's, and says of
//! every fact whether it is the same, differs or is on one side only;
//! `collect` logs in on the guest's console (a fake one here) and keeps
//! what is between the markers, and refuses a console that asks for a
//! password.

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/qemu-board-sysfs.py")
}

fn run(args: &[&str]) -> (bool, String) {
    let out = Command::new("python3")
        .arg("-I")
        .arg(script())
        .args(args)
        .output()
        .expect("python3 runs");
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

/// Atlas's class form, as listed on a board.
const BOARD_CLASSES: &str = "\n/sys/class/block:\ntotal 0\n\
lrwxrwxrwx    1 root     root             0 Nov  4 03:46 dm-0 -> ../../devices/virtual/block/dm-0\n\
lrwxrwxrwx    1 root     root             0 Nov  4 03:46 mmcblk0 -> ../../devices/platform/soc/1000fff000.mmc/mmc_host/mmc0/mmc0:0001/block/mmcblk0\n\
\n/sys/class/watchdog:\ntotal 0\n\
lrwxrwxrwx    1 root     root             0 Nov  4 03:46 watchdog0 -> ../../devices/platform/soc/107d200000.watchdog/bcm2835-wdt/watchdog/watchdog0\n\
/sys/class/watchdog/watchdog0/identity: Broadcom BCM2835 Watchdog timer\n\
/sys/class/watchdog/watchdog0/timeout: 15\n\
/sys/block/mmcblk0 force_ro: 0\n\
mmcblk0 size: 30535680\n";

/// Atlas's block form, of the same board.
const BOARD_BLOCKS: &str = "\n== dm-0 -> /sys/devices/virtual/block/dm-0\n  size=81992\n  dev=254:0\n\
== mmcblk0 -> /sys/devices/platform/soc/1000fff000.mmc/mmc_host/mmc0/mmc0:0001/block/mmcblk0\n\
  size=30535680\n  ro=0\n  device -> /sys/devices/platform/soc/1000fff000.mmc/mmc_host/mmc0/mmc0:0001\n\
/sys/devices/platform/soc/1000fff000.mmc/mmc_host/of_node\n\
block                    cid\n\
csd                      name\n";

/// The guest's, in collect's form.
const GUEST: &str = "/sys/class/block:\ntotal 0\n\
lrwxrwxrwx    1 0 0 0 Jan  1 00:31 dm-0 -> ../../devices/virtual/block/dm-0\n\
lrwxrwxrwx    1 0 0 0 Jan  1 00:31 mmcblk0 -> ../../devices/platform/soc/4010000000.mmc/pci0000:00/0000:00:01.0/mmc_host/mmc0/mmc0:0001/block/mmcblk0\n\
\n/sys/class/udc:\ntotal 0\n\
lrwxrwxrwx    1 0 0 0 Jan  1 00:31 1000480000.usb -> ../../devices/platform/axi/1000480000.usb/udc/1000480000.usb\n\
\n/sys/class/watchdog:\ntotal 0\n\
lrwxrwxrwx    1 0 0 0 Jan  1 00:31 watchdog0 -> ../../devices/platform/soc/107d200000.watchdog/bcm2835-wdt/watchdog/watchdog0\n\
/sys/class/watchdog/watchdog0/identity: i6300ESB timer\n\
/sys/class/watchdog/watchdog0/timeout: 15\n\
/sys/class/block/dm-0/size: 36024\n\
/sys/class/block/dm-0/dev: 254:0\n\
/sys/class/block/mmcblk0/size: 33554432\n\
/sys/class/block/mmcblk0/force_ro: 0\n\
/sys/class/block/mmcblk0/ro: 0\n\
card-attributes /sys/class/block/mmcblk0: name block csd cid \n";

#[test]
fn every_fact_is_the_same_differs_or_on_one_side() {
    let dir = tempfile::tempdir().expect("tempdir");
    let classes = dir.path().join("classes.txt");
    let blocks = dir.path().join("blocks.txt");
    let guest = dir.path().join("guest.txt");
    std::fs::write(&classes, BOARD_CLASSES).expect("write");
    std::fs::write(&blocks, BOARD_BLOCKS).expect("write");
    std::fs::write(&guest, GUEST).expect("write");
    let path = |p: &Path| p.to_str().expect("utf-8").to_string();
    let (ok, out) = run(&["diff", &path(&guest), &path(&classes), &path(&blocks)]);
    assert!(ok, "{out}");
    let lines: Vec<&str> = out.lines().collect();
    let has = |want: &str| assert!(lines.contains(&want), "no line {want:?} in:\n{out}");
    // Links compare by their target under /sys/devices, either form.
    has("same        block/dm-0 link: devices/virtual/block/dm-0");
    has("differs     block/mmcblk0 link:");
    has("  board  devices/platform/soc/1000fff000.mmc/mmc_host/mmc0/mmc0:0001/block/mmcblk0");
    has(
        "  guest  devices/platform/soc/4010000000.mmc/pci0000:00/0000:00:01.0/mmc_host/mmc0/mmc0:0001/block/mmcblk0",
    );
    // Atlas's "X size: N", "/sys/block/X attr: V" and "  key=value" lines.
    has("differs     block/mmcblk0 size:");
    has("same        block/mmcblk0 force_ro: 0");
    has("same        block/mmcblk0 ro: 0");
    has("same        block/dm-0 dev: 254:0");
    has("differs     block/dm-0 size:");
    // The card's attribute names, whatever their order or columns.
    has("same        block/mmcblk0 card attributes: block cid csd name");
    has("differs     watchdog/watchdog0 identity:");
    has("  board  Broadcom BCM2835 Watchdog timer");
    has("  guest  i6300ESB timer");
    has("same        watchdog/watchdog0 timeout: 15");
    has(
        "same        watchdog/watchdog0 link: devices/platform/soc/107d200000.watchdog/bcm2835-wdt/watchdog/watchdog0",
    );
    has(
        "guest only  udc/1000480000.usb link: devices/platform/axi/1000480000.usb/udc/1000480000.usb",
    );
    assert_eq!(
        lines.last().copied(),
        Some("7 same, 4 differs, 0 board only, 1 guest only"),
        "{out}"
    );
}

#[test]
fn board_listings_that_disagree_are_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let one = dir.path().join("one.txt");
    let two = dir.path().join("two.txt");
    std::fs::write(&one, BOARD_CLASSES).expect("write");
    std::fs::write(&two, BOARD_CLASSES.replace("timeout: 15", "timeout: 10")).expect("write");
    let path = |p: &Path| p.to_str().expect("utf-8").to_string();
    let (ok, out) = run(&["diff", &path(&one), &path(&one), &path(&two)]);
    assert!(!ok, "{out}");
    assert!(
        out.contains("the board listings disagree on watchdog/watchdog0 timeout: '15', and '10'"),
        "{out}"
    );
}

/// A guest's console on STATE/console.sock: a login prompt, then (unless
/// `password`) a shell that answers the listing command with `answer`.
/// Returns what collect typed.
fn fake_console(
    state: &Path,
    password: bool,
    answer: &'static str,
) -> std::thread::JoinHandle<String> {
    let listener = UnixListener::bind(state.join("console.sock")).expect("bind");
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().expect("accept");
        s.set_read_timeout(Some(Duration::from_millis(100)))
            .expect("timeout");
        s.write_all(b"\r\nWelcome\r\nbuildroot login: ")
            .expect("write");
        let mut got = Vec::new();
        let mut logged_in = false;
        let end = Instant::now() + Duration::from_secs(90);
        while Instant::now() < end {
            let mut chunk = [0u8; 4096];
            match s.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&chunk[..n]),
                Err(_) => {}
            }
            let text = String::from_utf8_lossy(&got).into_owned();
            if !logged_in && text.contains("root\r") {
                logged_in = true;
                let reply: &[u8] = if password {
                    b"\r\nPassword: "
                } else {
                    b"\r\n# "
                };
                s.write_all(reply).expect("write");
            }
            if let Some(at) = text.find("GAIA-SYSFS-''END\r") {
                // The shell echoes the command line (with the split
                // markers), then prints the listing.
                let start = text[..at]
                    .rfind("sh /tmp/gaia-sysfs.sh")
                    .expect("the command");
                let echo = text[start..at + "GAIA-SYSFS-''END".len()].to_string();
                let reply = format!("{echo}\r\nGAIA-SYSFS-BEGIN\r\n{answer}GAIA-SYSFS-END\r\n# ");
                s.write_all(reply.as_bytes()).expect("write");
                return text;
            }
        }
        String::from_utf8_lossy(&got).into_owned()
    })
}

#[test]
fn collect_keeps_what_the_guest_printed_between_the_markers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let console = fake_console(
        dir.path(),
        false,
        "/sys/class/udc:\r\ntotal 0\r\n\x1b[0mlrwxrwxrwx 1 0 0 0 Jan  1 00:31 u -> ../../devices/u\r\n",
    );
    let out = dir.path().join("guest.txt");
    let (ok, said) = run(&[
        "collect",
        dir.path().to_str().expect("utf-8"),
        out.to_str().expect("utf-8"),
    ]);
    let typed = console.join().expect("the console");
    assert!(ok, "{said}");
    // Logged in as root, wrote the listing script, ran it.
    assert!(typed.contains("root\r"), "{typed}");
    assert!(
        typed.contains("cat > /tmp/gaia-sysfs.sh << 'GAIA_SYSFS_EOF'\r"),
        "{typed}"
    );
    assert!(typed.contains("ls -l /sys/class/$c"), "{typed}");
    assert!(typed.contains("\rGAIA_SYSFS_EOF\r"), "{typed}");
    // Only the listing, with carriage returns and colours gone.
    assert_eq!(
        std::fs::read_to_string(&out).expect("the listing"),
        "/sys/class/udc:\ntotal 0\nlrwxrwxrwx 1 0 0 0 Jan  1 00:31 u -> ../../devices/u\n"
    );
}

#[test]
fn collect_refuses_a_console_that_asks_for_a_password() {
    let dir = tempfile::tempdir().expect("tempdir");
    let console = fake_console(dir.path(), true, "");
    let out = dir.path().join("guest.txt");
    let (ok, said) = run(&[
        "collect",
        dir.path().to_str().expect("utf-8"),
        out.to_str().expect("utf-8"),
    ]);
    assert!(!ok, "{said}");
    assert!(said.contains("asks for a password"), "{said}");
    assert!(!out.exists(), "a listing was written");
    drop(console);
}

#[test]
fn collect_gives_up_on_a_console_that_shows_no_prompt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let listener = UnixListener::bind(dir.path().join("console.sock")).expect("bind");
    // A guest still booting: its log has "# " and "login:" in lines, but
    // no prompt ends it, and it shows nothing back for what is typed.
    let console = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().expect("accept");
        s.write_all(b"[    0.000000] Linux version 7.3.5-v8 (gaia@gaia) # SMP PREEMPT\r\n[  OK  ] Listening on Orion login: helper socket.\r\n\x1b[0m")
            .expect("write");
        let mut got = Vec::new();
        let _ = s.read_to_end(&mut got);
        got
    });
    let out = dir.path().join("guest.txt");
    let (ok, said) = run(&[
        "collect",
        dir.path().to_str().expect("utf-8"),
        out.to_str().expect("utf-8"),
        "4",
    ]);
    assert!(!ok, "{said}");
    assert!(
        said.contains("no login or shell prompt on the guest's console in 4 s"),
        "{said}"
    );
    // It pressed Enter again while it waited, and typed nothing else.
    let typed = console.join().expect("the console");
    assert!(
        typed.len() >= 2 && typed.iter().all(|&b| b == b'\r'),
        "{typed:?}"
    );
    assert!(!out.exists(), "a listing was written");
}
