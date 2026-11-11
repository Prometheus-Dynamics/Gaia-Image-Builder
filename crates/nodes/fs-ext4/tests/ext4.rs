//! A tree as an ext4 image: the same bytes from two stores, clean under
//! `e2fsck -fn`, owners and modes as the entries say.

use gaia_fs_ext4::fs_ext4;
use gaia_step::Ctx;
use gaia_store::{Store, file_digest};
use gaia_types::{FileEntry, Image, Tree};
use std::path::Path;
use std::process::Command;

/// A program's path: Debian keeps e2fsprogs in /usr/sbin, Fedora in /usr/bin.
fn tool(name: &str) -> String {
    ["/usr/sbin", "/sbin", "/usr/bin"]
        .iter()
        .map(|dir| format!("{dir}/{name}"))
        .find(|p| Path::new(p).exists())
        .unwrap_or_default()
}

fn available() -> bool {
    let ok = ["mke2fs", "debugfs", "e2fsck"]
        .iter()
        .all(|p| !tool(p).is_empty());
    if !ok {
        assert!(
            std::env::var_os("GAIA_REQUIRE_SANDBOX").is_none(),
            "GAIA_REQUIRE_SANDBOX is set, and e2fsprogs is missing"
        );
        println!("SKIPPED: e2fsprogs (mke2fs, debugfs, e2fsck) is not installed");
    }
    ok
}

fn sample(dir: &Path) {
    let write = |p: &str, t: &str| {
        let p = dir.join(p);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(p, t).expect("write");
    };
    write("etc/hostname", "box\n");
    write("usr/bin/tool", "#!/bin/sh\necho tool\n");
    write("var/lib/camera/state", "0\n");
    write("usr/share/big", &"x".repeat(200_000));
    std::os::unix::fs::symlink("usr/bin", dir.join("bin")).expect("symlink");
}

fn entries(uid: u32) -> Vec<FileEntry> {
    let e = |path: &str, mode, uid, gid| FileEntry {
        path: path.into(),
        mode,
        uid,
        gid,
    };
    vec![
        e("usr/bin/tool", 0o4755, 0, 0),
        e("var/lib/camera", 0o750, uid, uid),
        e("var/lib/camera/state", 0o600, uid, uid),
    ]
}

fn image(root: &Path, uid: u32, size: u64) -> (Image, std::path::PathBuf) {
    let src = root.join("src");
    if !src.exists() {
        sample(&src);
    }
    let store_path = root.join("store");
    let store = Store::open(&store_path).expect("store");
    let tree = Tree {
        object: store.put_tree(&src).expect("put").into(),
        entries: entries(uid),
    };
    let image = fs_ext4(
        &store_path.display().to_string(),
        &tool("mke2fs"),
        &tool("debugfs"),
        tree,
        size,
        1024 * 1024,
        "rootfs",
        true,
        &Ctx::new(),
    )
    .expect("builds");
    let path = image.object.path_in(&store).expect("in store");
    (image, path)
}

fn stat(image: &Path, path: &str) -> String {
    let full = stat_full(image, path);
    assert!(
        full.contains("mtime: 0x00000000:00000000"),
        "{path}: {full}"
    );
    assert!(
        full.contains("ctime: 0x00000000:00000000"),
        "{path}: {full}"
    );
    full.lines().take(4).collect::<Vec<_>>().join(" | ")
}

fn stat_full(image: &Path, path: &str) -> String {
    let out = Command::new(tool("debugfs"))
        .args(["-R", &format!("stat {path}")])
        .arg(image)
        .output()
        .expect("debugfs");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn the_same_tree_gives_the_same_image_and_it_is_clean() {
    if !available() {
        return;
    }
    let (a, b) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let (first, path) = image(a.path(), 1000, 0);
    let (second, other) = image(b.path(), 1000, 0);
    let digest = file_digest(&path).expect("digest");
    println!("store a: {} {} bytes", digest, first.size.0);
    println!(
        "store b: {} {} bytes",
        file_digest(&other).expect("digest"),
        second.size.0
    );
    assert_eq!(digest, file_digest(&other).expect("digest"));

    let fsck = Command::new(tool("e2fsck"))
        .arg("-fn")
        .arg(&path)
        .output()
        .expect("e2fsck");
    let report = String::from_utf8_lossy(&fsck.stdout);
    println!(
        "e2fsck -fn: exit {} | {}",
        fsck.status.code().unwrap_or(-1),
        report.trim().lines().last().unwrap_or("")
    );
    assert!(
        fsck.status.success(),
        "{report}{}",
        String::from_utf8_lossy(&fsck.stderr)
    );

    for (inside, want) in [
        (
            "/usr/bin/tool",
            ["Mode:  04755", "User:     0", "Group:     0"],
        ),
        (
            "/var/lib/camera",
            ["Mode:  0750", "User:  1000", "Group:  1000"],
        ),
        (
            "/var/lib/camera/state",
            ["Mode:  0600", "User:  1000", "Group:  1000"],
        ),
        (
            "/etc/hostname",
            ["Mode:  0644", "User:     0", "Group:     0"],
        ),
    ] {
        let shown = stat(&path, inside);
        println!("{inside}: {shown}");
        for w in want {
            assert!(shown.contains(w), "{inside}: want {w:?} in {shown}");
        }
        assert!(
            shown.contains("ctime: 0x00000000") || !shown.contains("ctime"),
            "{shown}"
        );
    }

    // An owner changed: another key, another image.
    let c = tempfile::tempdir().expect("tempdir");
    let (_, changed) = image(c.path(), 1001, 0);
    let changed = file_digest(&changed).expect("digest");
    println!("uid 1001: {changed}");
    assert_ne!(changed, digest);
}

#[test]
fn a_fixed_size_is_kept_and_too_small_fails() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let (image_, _) = image(dir.path(), 1000, 64 * 1024 * 1024);
    assert_eq!(image_.size.0, 64 * 1024 * 1024);
    let small = tempfile::tempdir().expect("tempdir");
    let src = small.path().join("src");
    sample(&src);
    let store_path = small.path().join("store");
    let store = Store::open(&store_path).expect("store");
    let tree = Tree {
        object: store.put_tree(&src).expect("put").into(),
        entries: entries(0),
    };
    let error = fs_ext4(
        &store_path.display().to_string(),
        &tool("mke2fs"),
        &tool("debugfs"),
        tree,
        64 * 1024,
        0,
        "",
        false,
        &Ctx::new(),
    )
    .expect_err("too small");
    println!("64 KiB: {error}");
}
