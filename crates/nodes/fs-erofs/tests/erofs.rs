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
        Some(&tool("mkfs.erofs")),
        None,
        tree,
        compression,
        0,
        None,
        None,
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
        Some(&tool("mkfs.erofs")),
        None,
        tree,
        "brotli",
        0,
        None,
        None,
        &Ctx::new(),
    )
    .expect_err("unknown");
    println!("{error}");
    assert!(error.to_string().contains("not one of"));
}

/// The image's kernel must be able to open what is made: the option for
/// the compression, built in, in its final configuration. Checked before
/// anything is made, and again when the image is found in the store.
#[test]
fn a_kernel_that_cannot_read_the_compression_fails_the_step() {
    use gaia_fs_erofs::{kernel_options, kernel_reads};
    // As a trimmed kernel had it: lzma and no zstd.
    let config = "CONFIG_EROFS_FS=y\nCONFIG_EROFS_FS_ZIP=y\nCONFIG_EROFS_FS_ZIP_LZMA=y\n# CONFIG_EROFS_FS_ZIP_ZSTD is not set\nCONFIG_EROFS_FS_ZIP_DEFLATE=m\n";
    assert_eq!(kernel_reads(config, "lzma"), Ok(()));
    assert_eq!(kernel_reads(config, "lz4"), Ok(()));
    assert_eq!(kernel_reads(config, "lz4hc"), Ok(()));
    assert_eq!(kernel_reads(config, "none"), Ok(()));
    let why = kernel_reads(config, "zstd").expect_err("no zstd");
    assert!(
        why.starts_with("CONFIG_EROFS_FS_ZIP_ZSTD is `not set` in the kernel's .config")
            && why.ends_with("an erofs root (zstd) needs =y")
            && "gaia.fs.erofs: ".len() + why.len() <= 120,
        "{why}"
    );
    // A module is not built in: a root is opened before modules load.
    let why = kernel_reads(config, "deflate").expect_err("a module");
    assert!(why.contains("CONFIG_EROFS_FS_ZIP_DEFLATE is `m`"), "{why}");
    // No erofs at all, and no line for it.
    let why = kernel_reads("CONFIG_EXT4_FS=y\n", "none").expect_err("no erofs");
    assert!(why.contains("CONFIG_EROFS_FS is `not set`"), "{why}");
    assert_eq!(kernel_options("none"), ["CONFIG_EROFS_FS"]);

    if !available() || !has("zstd") {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    sample(&src);
    let store_path = dir.path().join("store");
    let store = Store::open(&store_path).expect("store");
    let tree = || Tree {
        object: store.put_tree(&src).expect("put").into(),
        entries: Vec::new(),
    };
    let kernel = |text: &str| {
        let build = dir.path().join("kernel-build");
        std::fs::create_dir_all(&build).expect("mkdir");
        std::fs::write(build.join(".config"), text).expect("write");
        Tree {
            object: store.put_tree(&build).expect("put").into(),
            entries: Vec::new(),
        }
    };
    let make = |compression: &str, cluster: i64, kernel: Option<&Tree>| {
        fs_erofs(
            &store_path.display().to_string(),
            Some(&tool("mkfs.erofs")),
            None,
            tree(),
            compression,
            0,
            (cluster != 0).then_some(&cluster),
            kernel,
            &Ctx::new(),
        )
        .map_err(|e| e.to_string())
    };
    let without = kernel(config);
    let why = make("zstd", 0, Some(&without)).expect_err("the kernel has no zstd");
    println!("{why}");
    assert!(
        why.contains("CONFIG_EROFS_FS_ZIP_ZSTD is `not set`"),
        "{why}"
    );
    // Made for a kernel that reads it; then the same image, found in the
    // store, is refused all the same for a kernel that does not.
    let with = kernel(&config.replace(
        "# CONFIG_EROFS_FS_ZIP_ZSTD is not set",
        "CONFIG_EROFS_FS_ZIP_ZSTD=y",
    ));
    let made = make("zstd", 0, Some(&with)).expect("the kernel reads zstd");
    assert!(make("zstd", 0, Some(&without)).is_err());
    // Bigger clusters are another image; a size that is not blocks is refused.
    let big = make("zstd", 65536, Some(&with)).expect("64 KiB clusters");
    assert_ne!(big.object, made.object);
    println!(
        "4 KiB clusters {} bytes, 64 KiB clusters {} bytes",
        made.size.0, big.size.0
    );
    let why = make("zstd", 5000, None).expect_err("not blocks");
    assert!(
        why.contains("cluster 5000 is not a number of 4096-byte blocks"),
        "{why}"
    );
    let why = make("none", 65536, None).expect_err("no compression");
    assert!(why.contains("clusters are of compressed data"), "{why}");
}
