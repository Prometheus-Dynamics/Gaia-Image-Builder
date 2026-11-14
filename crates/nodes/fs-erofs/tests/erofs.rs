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
        None,
        None,
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
        None,
        None,
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
            None,
            None,
            None,
            None,
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

/// The ways of packing an image beyond its algorithm: asked for by name,
/// held to the kernel's version, and part of what the image is.
#[test]
fn packing_options_are_passed_held_to_the_kernel_and_keyed() {
    use gaia_fs_erofs::{kernel_new_enough, kernel_version};
    assert_eq!(
        kernel_version("# SPDX\nVERSION = 6\nPATCHLEVEL = 12\nSUBLEVEL = 3\n"),
        Some((6, 12))
    );
    assert_eq!(kernel_version("all:\n\techo\n"), None);
    assert_eq!(
        kernel_new_enough((6, 1), &["lzma", "fragments", "dedupe"]),
        Ok(())
    );
    assert_eq!(
        kernel_new_enough((5, 15), &["lzma", "big clusters", "fragments", "lz4"]),
        Err(
            "the image's kernel is 5.15; of its erofs, lzma needs 5.16, fragments needs 6.1".into()
        )
    );

    if !available() || !has("lzma") {
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
    let kernel = |version: &str| {
        let build = dir.path().join(format!("kernel-{version}"));
        std::fs::create_dir_all(&build).expect("mkdir");
        let (major, minor) = version.split_once('.').expect("a version");
        std::fs::write(
            build.join("Makefile"),
            format!("VERSION = {major}\nPATCHLEVEL = {minor}\n"),
        )
        .expect("write");
        std::fs::write(
            build.join(".config"),
            "CONFIG_EROFS_FS=y\nCONFIG_EROFS_FS_ZIP=y\nCONFIG_EROFS_FS_ZIP_LZMA=y\n",
        )
        .expect("write");
        Tree {
            object: store.put_tree(&build).expect("put").into(),
            entries: Vec::new(),
        }
    };
    let make = |compression: &str, dictsize: i64, options: [bool; 3], kernel: Option<&Tree>| {
        let on = |n: usize| options[n].then_some(&true);
        fs_erofs(
            &store_path.display().to_string(),
            Some(&tool("mkfs.erofs")),
            None,
            tree(),
            compression,
            6,
            Some(&65536),
            (dictsize != 0).then_some(&dictsize),
            on(0),
            on(1),
            on(2),
            kernel,
            &Ctx::new(),
        )
        .map_err(|e| e.to_string())
    };
    let none = [false; 3];
    let plain = make("lzma", 0, none, None).expect("plain");
    // Fragments and tail packing, with a smaller dictionary.
    let packed = make("lzma", 1 << 20, [true, true, false], None).expect("packed");
    assert_ne!(packed.object, plain.object, "another image, another key");
    println!(
        "plain {} bytes, packed {} bytes",
        plain.size.0, packed.size.0
    );
    let dump = Command::new(tool("dump.erofs"))
        .arg("-s")
        .arg(packed.object.path_in(&store).expect("in store"))
        .output()
        .expect("dump.erofs");
    let text = String::from_utf8_lossy(&dump.stdout);
    assert!(text.contains("fragments"), "{text}");
    // A kernel too old for what was asked.
    let old = kernel("5.16");
    let why = make("lzma", 0, [true, false, false], Some(&old)).expect_err("5.16");
    assert!(
        why.contains("the image's kernel is 5.16; of its erofs, fragments needs 6.1"),
        "{why}"
    );
    let new = kernel("6.12");
    make("lzma", 0, [true, false, true], Some(&new)).expect("6.12 reads them");
    // What does not go together.
    let why = make("lz4", 1 << 20, none, None).expect_err("a dictionary for lz4");
    assert!(why.contains("dictsize 1048576 is for lzma"), "{why}");
    let why = fs_erofs(
        &store_path.display().to_string(),
        Some(&tool("mkfs.erofs")),
        None,
        tree(),
        "none",
        0,
        None,
        None,
        Some(&true),
        None,
        None,
        None,
        &Ctx::new(),
    )
    .expect_err("fragments of nothing compressed")
    .to_string();
    assert!(
        why.contains("fragments is set and compression is none"),
        "{why}"
    );
}

/// A tool that says it made the image and did not put the tree's bytes in
/// it (as a real one did, with one of its options): the step reads back
/// what was made, and fails.
#[test]
fn an_image_that_does_not_hold_the_tree_fails_whatever_the_tool_said() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    sample(&src);
    let store_path = dir.path().join("store");
    let store = Store::open(&store_path).expect("store");
    // A mkfs.erofs that makes a good image of an empty directory, and exits
    // 0; the real fsck.erofs beside it.
    let tools = dir.path().join("tools");
    let empty = dir.path().join("empty");
    std::fs::create_dir_all(&tools).expect("mkdir");
    std::fs::create_dir_all(&empty).expect("mkdir");
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in --version) exec {real} --version ;; esac\nfor last in \"$@\"; do prev=$image; image=$last; done\nexec {real} --quiet \"$prev\" {empty}\n",
        real = tool("mkfs.erofs"),
        empty = empty.display()
    );
    let lying = tools.join("mkfs.erofs");
    std::fs::write(&lying, script).expect("write");
    std::fs::set_permissions(&lying, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");
    std::os::unix::fs::symlink(tool("fsck.erofs"), tools.join("fsck.erofs")).expect("symlink");
    let tree = Tree {
        object: store.put_tree(&src).expect("put").into(),
        entries: Vec::new(),
    };
    let why = fs_erofs(
        &store_path.display().to_string(),
        Some(&lying.display().to_string()),
        None,
        tree,
        "none",
        0,
        None,
        None,
        None,
        None,
        None,
        None,
        &Ctx::new(),
    )
    .expect_err("the image is of nothing")
    .to_string();
    println!("{why}");
    assert!(
        why.contains("the image made does not hold the tree:")
            && why.contains("of its files differ when read back"),
        "{why}"
    );
}
