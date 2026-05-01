use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_path(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir()
        .join("gaia-tests")
        .join(format!("{prefix}-{nonce}"))
}

fn create_tar_from_dir(rootfs_dir: &Path, archive_path: &Path) {
    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent).expect("archive parent");
    }
    let status = Command::new("tar")
        .arg("-cf")
        .arg(archive_path)
        .arg("-C")
        .arg(rootfs_dir)
        .arg(".")
        .status()
        .expect("tar create");
    assert!(status.success(), "tar creation should succeed");
}

fn tar_listing(archive_path: &Path) -> String {
    let output = Command::new("tar")
        .arg("-tf")
        .arg(archive_path)
        .output()
        .expect("tar listing");
    assert!(output.status.success(), "tar listing should succeed");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn tar_extract_text(archive_path: &Path, entry: &str) -> String {
    let output = Command::new("tar")
        .arg("-xOf")
        .arg(archive_path)
        .arg(entry)
        .output()
        .expect("tar extract");
    assert!(output.status.success(), "tar extraction should succeed");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn assert_tar_contains_paths(archive_path: &Path, expected_paths: &[&str]) {
    let listing = tar_listing(archive_path);
    for expected_path in expected_paths {
        assert!(
            listing.contains(expected_path),
            "expected archive '{}' to contain '{}'",
            archive_path.display(),
            expected_path
        );
    }
}

#[test]
fn final_tar_verification_helpers_detect_expected_runtime_paths() {
    let rootfs_dir = temp_path("gaia-image-verify-rootfs");
    let archive_path = temp_path("gaia-image-verify-rootfs.tar");
    fs::create_dir_all(rootfs_dir.join("usr/bin")).expect("bin dir");
    fs::create_dir_all(rootfs_dir.join("etc/default")).expect("env dir");
    fs::create_dir_all(rootfs_dir.join("etc/systemd/system")).expect("service dir");
    fs::write(rootfs_dir.join("usr/bin/smoke-app"), "binary").expect("binary");
    fs::write(rootfs_dir.join("etc/motd"), "hello motd").expect("motd");
    fs::write(rootfs_dir.join("etc/default/runtime.env"), "MODE=smoke\n").expect("env");
    fs::write(
        rootfs_dir.join("etc/systemd/system/gaia-smoke.service"),
        "[Service]\nExecStart=/usr/bin/smoke-app\n",
    )
    .expect("service");

    create_tar_from_dir(&rootfs_dir, &archive_path);

    assert_tar_contains_paths(
        &archive_path,
        &[
            "./usr/bin/smoke-app",
            "./etc/motd",
            "./etc/default/runtime.env",
            "./etc/systemd/system/gaia-smoke.service",
        ],
    );
    assert_eq!(tar_extract_text(&archive_path, "./etc/motd"), "hello motd");
    assert_eq!(
        tar_extract_text(&archive_path, "./etc/default/runtime.env"),
        "MODE=smoke\n"
    );
}

#[test]
fn final_tar_verification_helpers_catch_missing_runtime_paths() {
    let rootfs_dir = temp_path("gaia-image-verify-missing-rootfs");
    let archive_path = temp_path("gaia-image-verify-missing-rootfs.tar");
    fs::create_dir_all(rootfs_dir.join("etc")).expect("etc dir");
    fs::write(rootfs_dir.join("etc/motd"), "hello motd").expect("motd");

    create_tar_from_dir(&rootfs_dir, &archive_path);

    let listing = tar_listing(&archive_path);
    assert!(listing.contains("./etc/motd"));
    assert!(!listing.contains("./usr/bin/smoke-app"));
}
