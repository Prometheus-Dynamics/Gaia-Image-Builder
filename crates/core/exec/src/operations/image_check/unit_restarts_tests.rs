//! The restart policy warnings, on a root filesystem of unit files.

use super::unit_restarts::{findings, parse_seconds};
use gaia_testkit::TempDir;
use std::collections::BTreeSet;
use std::path::Path;

fn put(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(path, text).expect("write");
}

fn enable(root: &Path, unit: &str) {
    let wants = root.join("etc/systemd/system/multi-user.target.wants");
    std::fs::create_dir_all(&wants).expect("wants");
    std::os::unix::fs::symlink(format!("/usr/lib/systemd/system/{unit}"), wants.join(unit))
        .expect("link");
}

fn codes(root: &Path, skip: &[&str]) -> Vec<(String, &'static str)> {
    let skip: BTreeSet<String> = skip.iter().map(|name| name.to_string()).collect();
    findings(root, &skip)
        .into_iter()
        .map(|finding| (finding.unit, finding.code))
        .collect()
}

#[test]
fn restart_without_start_limit_off_slow_restarts_and_no_restart_are_reported() {
    let scratch = TempDir::new("gaia-image-check-units");
    let root = scratch.path();
    let units = "usr/lib/systemd/system";
    // Restarts, keeps the default limit.
    put(
        root,
        &format!("{units}/limited.service"),
        "[Service]\nExecStart=/x\nRestart=always\nRestartSec=1\n",
    );
    // Restarts forever, 2 s apart.
    put(
        root,
        &format!("{units}/good.service"),
        "[Unit]\nStartLimitIntervalSec=0\n[Service]\nExecStart=/x\nRestart=always\nRestartSec=2s\n",
    );
    // A drop-in turns the limit off; the unit waits a minute.
    put(
        root,
        &format!("{units}/slow.service"),
        "[Service]\nExecStart=/x\nRestart=on-failure\nRestartSec=1min\n",
    );
    put(
        root,
        &format!("{units}/slow.service.d/limit.conf"),
        "[Unit]\nStartLimitIntervalSec=0\n",
    );
    // Enabled, long-running, no Restart=.
    put(
        root,
        &format!("{units}/norestart.service"),
        "[Service]\nExecStart=/x\n",
    );
    enable(root, "norestart.service");
    // Not enabled, or a oneshot: left alone.
    put(
        root,
        &format!("{units}/idle.service"),
        "[Service]\nExecStart=/x\n",
    );
    put(
        root,
        &format!("{units}/setup.service"),
        "[Service]\nType=oneshot\nExecStart=/x\n",
    );
    enable(root, "setup.service");
    // systemd's own unit, and a masked one.
    put(
        root,
        &format!("{units}/systemd-x.service"),
        "[Service]\nExecStart=/x\nRestart=always\n",
    );
    put(
        root,
        &format!("{units}/masked.service"),
        "[Service]\nExecStart=/x\nRestart=always\n",
    );
    std::fs::create_dir_all(root.join("etc/systemd/system")).expect("etc");
    std::os::unix::fs::symlink("/dev/null", root.join("etc/systemd/system/masked.service"))
        .expect("mask");

    assert_eq!(
        codes(root, &["systemd-x.service"]),
        vec![
            ("limited.service".to_string(), "unit_start_limit"),
            ("norestart.service".to_string(), "unit_no_restart"),
            ("slow.service".to_string(), "unit_restart_slow"),
        ]
    );

    // The manager's default turns every unit's limit off.
    put(
        root,
        "etc/systemd/system.conf.d/limits.conf",
        "[Manager]\nDefaultStartLimitIntervalSec=0\n",
    );
    assert_eq!(
        codes(root, &["systemd-x.service"]),
        vec![
            ("norestart.service".to_string(), "unit_no_restart"),
            ("slow.service".to_string(), "unit_restart_slow"),
        ]
    );
}

#[test]
fn time_spans_parse_like_systemd() {
    assert_eq!(parse_seconds("10"), Some(10.0));
    assert_eq!(parse_seconds("500ms"), Some(0.5));
    assert_eq!(parse_seconds("1min 30s"), Some(90.0));
    assert_eq!(parse_seconds("2h"), Some(7200.0));
    assert_eq!(parse_seconds("infinity"), None);
    assert_eq!(parse_seconds("soon"), None);
}

/// The findings on a real target tree: `GAIA_UNITS_ROOTFS=<output>/target`.
#[test]
#[ignore]
fn findings_on_a_real_tree() {
    let Ok(rootfs) = std::env::var("GAIA_UNITS_ROOTFS") else {
        return;
    };
    let rootfs = Path::new(&rootfs);
    let list =
        std::fs::read_to_string(rootfs.join("../build/packages-file-list.txt")).unwrap_or_default();
    let skip: BTreeSet<String> = list
        .lines()
        .filter_map(|line| line.split_once(','))
        .filter(|(pkg, path)| *pkg == "systemd" && path.contains("/systemd/system/"))
        .filter_map(|(_, path)| path.rsplit('/').next().map(str::to_string))
        .collect();
    for finding in findings(rootfs, &skip) {
        println!("{} {}", finding.code, finding.message);
    }
}
