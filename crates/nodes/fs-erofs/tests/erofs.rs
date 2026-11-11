//! A tree as an EROFS image: the same bytes from two stores, clean under
//! `fsck.erofs`, owners and modes as the entries say, sizes by compressor.
//! Without erofs-utils the tests say SKIPPED, unless `GAIA_REQUIRE_TOOLS=1`,
//! when they fail.

use gaia_fs_erofs::fs_erofs;
use gaia_step::Ctx;
use gaia_store::{Store, file_digest};
use gaia_types::{FileEntry, Tree};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A program's path, in the usual places.
fn tool(name: &str) -> String {
    ["/usr/bin", "/usr/sbin", "/sbin"]
        .iter()
        .map(|dir| format!("{dir}/{name}"))
        .find(|p| Path::new(p).exists())
        .unwrap_or_default()
}

fn available() -> bool {
    let ok = !tool("mkfs.erofs").is_empty() && !tool("fsck.erofs").is_empty();
    if !ok {
        assert!(
            std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
            "GAIA_REQUIRE_TOOLS is set, and erofs-utils is missing"
        );
        println!("SKIPPED: erofs-utils (mkfs.erofs, fsck.erofs) is not installed");
    }
    ok
}

/// Whether this mkfs.erofs was built with `compressor`.
fn has(compressor: &str) -> bool {
    let out = Command::new(tool("mkfs.erofs"))
        .arg("--help")
        .output()
        .expect("help");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    text.lines().any(|l| l.trim() == compressor)
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
    let text: String = (0..20_000)
        .map(|i| format!("line {i} of a compressible file\n"))
        .collect();
    write("usr/share/doc/big.txt", &text);
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

fn image(root: &Path, uid: u32, compression: &str) -> (u64, PathBuf) {
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
    let image = fs_erofs(
        &store_path.display().to_string(),
        &tool("mkfs.erofs"),
        tree,
        compression,
        0,
        &Ctx::new(),
    )
    .expect("builds");
    (
        image.size.0,
        image.object.path_in(&store).expect("in store"),
    )
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
    let (size, path) = image(a.path(), 1000, "lz4hc");
    let (_, other) = image(b.path(), 1000, "lz4hc");
    let digest = file_digest(&path).expect("digest");
    println!("lz4hc, store a: {digest} {size} bytes");
    println!("lz4hc, store b: {}", file_digest(&other).expect("digest"));
    assert_eq!(digest, file_digest(&other).expect("digest"));

    let x = a.path().join("extracted");
    let fsck = Command::new(tool("fsck.erofs"))
        .arg(format!("--extract={}", x.display()))
        .arg("--preserve-perms")
        .arg(&path)
        .output()
        .expect("fsck.erofs");
    println!("fsck.erofs: exit {}", fsck.status.code().unwrap_or(-1));
    assert!(
        fsck.status.success(),
        "{}",
        String::from_utf8_lossy(&fsck.stderr)
    );

    // Modes as the entries say (owners need root to extract; dump.erofs
    // reads them).
    for (inside, mode) in [
        ("usr/bin/tool", 0o4755),
        ("var/lib/camera", 0o750),
        ("var/lib/camera/state", 0o600),
        ("etc/hostname", 0o644),
    ] {
        let got = std::fs::symlink_metadata(x.join(inside))
            .expect("extracted")
            .mode()
            & 0o7777;
        assert_eq!(got, mode, "{inside}");
    }
    if !tool("dump.erofs").is_empty() {
        for (inside, want) in [
            ("/usr/bin/tool", "Uid: 0   Gid: 0"),
            ("/var/lib/camera/state", "Uid: 1000   Gid: 1000"),
        ] {
            let out = Command::new(tool("dump.erofs"))
                .arg(format!("--path={inside}"))
                .arg(&path)
                .output()
                .expect("dump");
            let text = String::from_utf8_lossy(&out.stdout);
            let line = text
                .lines()
                .find(|l| l.starts_with("Uid:"))
                .unwrap_or_default();
            println!("{inside}: {line}");
            assert!(line.contains(want), "{inside}: {text}");
            assert!(
                text.contains("Timestamp: 1970-01-01 00:00:00") || text.contains("1969-12-31"),
                "{text}"
            );
        }
    }

    // An owner changed: another image.
    let c = tempfile::tempdir().expect("tempdir");
    let (_, changed) = image(c.path(), 1001, "lz4hc");
    let changed = file_digest(&changed).expect("digest");
    println!("uid 1001: {changed}");
    assert_ne!(changed, digest);
}

#[test]
fn sizes_by_compression() {
    if !available() {
        return;
    }
    for compression in ["none", "lz4", "lz4hc", "lzma", "zstd"] {
        if compression != "none" && !has(compression) {
            println!("{compression:<6} SKIPPED: this mkfs.erofs was built without it");
            continue;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let (size, path) = image(dir.path(), 1000, compression);
        let fsck = Command::new(tool("fsck.erofs"))
            .arg(&path)
            .status()
            .expect("fsck");
        assert!(fsck.success(), "{compression}: fsck.erofs failed");
        println!("{compression:<6} {size:>8} bytes  fsck.erofs clean");
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    sample(&src);
    let store_path = dir.path().join("store");
    let store = Store::open(&store_path).expect("store");
    let tree = Tree {
        object: store.put_tree(&src).expect("put").into(),
        entries: Vec::new(),
    };
    let error = fs_erofs(
        &store_path.display().to_string(),
        &tool("mkfs.erofs"),
        tree,
        "brotli",
        0,
        &Ctx::new(),
    )
    .expect_err("unknown");
    println!("{error}");
    assert!(error.to_string().contains("not one of"));
}
