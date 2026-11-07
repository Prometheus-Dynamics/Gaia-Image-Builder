//! Tests for leaving the D-Bus daemon out: the generated script runs with `sh`
//! on a fake output tree, as Buildroot would run it.
use super::*;
use gaia_testkit::TempDir;
use std::process::Command;

fn put(root: &Path, rel: &str, contents: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("parent dirs");
    fs::write(path, contents).expect("write");
}

const DROPS: RootfsDrops = RootfsDrops {
    shadowed_modules: false,
    dbus_daemon: true,
};

/// A Raze-like tree: dbus, systemd's bus files, dnsmasq's policy, zlib's
/// library that dbus lists too.
fn tree(tag: &str) -> TempDir {
    let dir = TempDir::new(&format!("gaia-rootfs-no-dbus-{tag}"));
    let root = dir.path();
    let files = [
        ("dbus", "usr/bin/dbus-daemon"),
        ("dbus", "usr/bin/dbus-send"),
        ("dbus", "usr/libexec/dbus-daemon-launch-helper"),
        ("dbus", "usr/lib/libdbus-1.so.3.38.3"),
        ("dbus", "usr/lib/libdbus-1.so"),
        ("dbus", "usr/lib/libz.so.1"),
        ("dbus", "usr/share/dbus-1/system.conf"),
        ("dbus", "etc/dbus-1/system.conf"),
        ("dbus", "usr/lib/systemd/system/dbus.service"),
        ("dbus", "usr/lib/systemd/system/dbus.socket"),
        ("dbus", "usr/lib/sysusers.d/dbus.conf"),
        ("zlib", "usr/lib/libz.so.1"),
        ("dnsmasq", "usr/sbin/dnsmasq"),
        ("dnsmasq", "usr/share/dbus-1/system.d/dnsmasq.conf"),
        (
            "systemd",
            "usr/share/dbus-1/system-services/org.freedesktop.hostname1.service",
        ),
        ("systemd", "usr/lib/systemd/system/systemd-networkd.service"),
    ];
    let mut list = String::new();
    for (pkg, rel) in files {
        list.push_str(&format!("{pkg},./{rel}\n"));
        put(root, &format!("target/{rel}"), b"x");
        put(
            root,
            &format!("per-package/{pkg}/target/{rel}"),
            &[0u8; 1024],
        );
    }
    put(root, "build/packages-file-list.txt", list.as_bytes());
    let wants = root.join("target/etc/systemd/system/sockets.target.wants");
    fs::create_dir_all(&wants).expect("wants dir");
    std::os::unix::fs::symlink(
        "/usr/lib/systemd/system/dbus.socket",
        wants.join("dbus.socket"),
    )
    .expect("preset link");
    put(
        root,
        "target/usr/lib/systemd/system/systemd-networkd.service",
        b"[Unit]\nAfter=dbus.service\nWants=dbus.socket\n[Service]\nType=notify\nBusName=org.freedesktop.network1\n",
    );
    dir
}

fn run(root: &Path) -> StagedRootfsExclude {
    let staged = stage_rootfs_exclude(root, &[], DROPS)
        .expect("stage")
        .expect("staged");
    let status = Command::new("sh")
        .arg(&staged.script)
        .arg(root.join("target"))
        .status()
        .expect("sh runs the script");
    assert!(status.success(), "the post-build script failed");
    staged
}

#[cfg(unix)]
#[test]
fn drops_the_daemon_its_units_and_the_bus_policies_and_keeps_libdbus() {
    let dir = tree("drop");
    let root = dir.path();
    let summary = run(root).verify().expect("the removal verifies");
    let target = root.join("target");
    for gone in [
        "usr/bin/dbus-daemon",
        "usr/bin/dbus-send",
        "usr/libexec",
        "usr/lib/libdbus-1.so",
        "usr/share/dbus-1",
        "etc/dbus-1",
        "usr/lib/systemd/system/dbus.service",
        "usr/lib/systemd/system/dbus.socket",
        "etc/systemd/system/sockets.target.wants",
    ] {
        assert!(
            fs::symlink_metadata(target.join(gone)).is_err(),
            "{gone} stays"
        );
    }
    for kept in [
        "usr/lib/libdbus-1.so.3.38.3",
        "usr/lib/libz.so.1",
        "usr/sbin/dnsmasq",
        "usr/lib/systemd/system/systemd-networkd.service",
    ] {
        assert!(target.join(kept).is_file(), "{kept} is removed");
    }
    // 12 rows: 9 of dbus (libz is shared), 3 bus files, the preset link.
    let (files, bytes) = summary.no_dbus.expect("on");
    assert_eq!(files, 12);
    assert_eq!(bytes, 11 * 1024, "the preset link has no per-package file");
    assert_eq!(summary.shared, vec!["usr/lib/libz.so.1".to_string()]);
    assert!(summary.names.is_empty());
    assert_eq!(
        summary.no_dbus_message().as_deref(),
        Some("rootfs: dropped the D-Bus daemon (libdbus-1 kept): 12 files, 11.0 KiB")
    );
}

#[cfg(unix)]
#[test]
fn a_unit_that_needs_the_bus_refuses_the_image() {
    let dir = tree("refuse");
    let root = dir.path();
    put(
        root,
        "target/usr/lib/systemd/system/avahi-daemon.service",
        b"[Service]\nType=dbus\nBusName=org.freedesktop.Avahi\n",
    );
    let error = run(root).verify().expect_err("avahi needs the daemon");
    assert!(
        error.message.contains(
            "image.rootfs.drop_dbus_daemon: 'usr/lib/systemd/system/avahi-daemon.service' is Type=dbus"
        ),
        "{}",
        error.message
    );
    assert_eq!(
        bus_need("[Unit]\nRequires=network.target dbus.socket\n").as_deref(),
        Some("has Requires=network.target dbus.socket")
    );
    assert_eq!(
        bus_need("[Unit]\nWants=dbus.socket\nAfter=dbus.service\n"),
        None
    );
}

#[test]
fn the_drop_changes_the_digest_and_the_shadowed_one_is_unchanged() {
    let none = staged_digest(&[], RootfsDrops::default());
    assert_eq!(none, "");
    let dbus = staged_digest(&[], DROPS);
    assert_ne!(dbus, "");
    let shadowed = RootfsDrops {
        shadowed_modules: true,
        dbus_daemon: false,
    };
    let both = RootfsDrops {
        shadowed_modules: true,
        dbus_daemon: true,
    };
    assert_ne!(staged_digest(&[], shadowed), dbus);
    assert_ne!(staged_digest(&[], both), staged_digest(&[], shadowed));
}

#[test]
fn the_make_blanks_the_permissions_of_what_is_removed() {
    let dir = TempDir::new("gaia-rootfs-no-dbus-make");
    let root = dir.path();
    let staged = stage_rootfs_exclude(root, &["libcamera-apps".to_string()], DROPS)
        .expect("stage")
        .expect("staged");
    assert_eq!(
        staged.make_overrides(),
        ["LIBCAMERA_APPS_PERMISSIONS=", "DBUS_PERMISSIONS="]
    );
}
