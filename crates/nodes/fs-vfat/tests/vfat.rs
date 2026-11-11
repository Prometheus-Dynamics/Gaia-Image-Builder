//! A tree as a FAT image: the same bytes from two stores with different
//! file times, clean under `fsck.vfat -n`, the contents read back, and the
//! FAT type asked for. Without mtools and dosfstools the tests say SKIPPED,
//! unless `GAIA_REQUIRE_TOOLS=1`, when they fail.

use gaia_fs_vfat::fs_vfat;
use gaia_step::Ctx;
use gaia_store::{Store, file_digest};
use gaia_types::Tree;
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
    let ok = ["mformat", "mcopy", "fsck.vfat"]
        .iter()
        .all(|name| !tool(name).is_empty());
    if !ok {
        assert!(
            std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
            "GAIA_REQUIRE_TOOLS is set, and mtools or dosfstools is missing"
        );
        println!("SKIPPED: mtools (mformat, mcopy) and dosfstools (fsck.vfat) are not installed");
    }
    ok
}

/// A small boot-like tree: files, a directory and a link.
fn sample(dir: &Path, mtime_secs: u64) {
    let write = |p: &str, t: &str| {
        let p = dir.join(p);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(p, t).expect("write");
    };
    write("config.txt", "dtoverlay=example\n");
    write("overlays/a.dtbo", "overlay bytes\n");
    write("kernel.img", &"kernel line\n".repeat(3000));
    std::os::unix::fs::symlink("config.txt", dir.join("link.txt")).expect("symlink");
    // Every file and directory gets the same time, so the time is the only
    // thing that could differ between two copies of the tree.
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime_secs);
    for path in walk(dir) {
        let file = std::fs::File::open(&path).expect("open");
        file.set_times(std::fs::FileTimes::new().set_modified(time))
            .expect("set time");
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            out.extend(walk(&path));
        }
        out.push(path);
    }
    out
}

/// The image of a copy of [`sample`] made in its own store, with
/// `mtime_secs` as the time of every entry, and `size` and `fat_size`.
fn image(root: &Path, mtime_secs: u64, size: u64, fat_size: u32) -> (PathBuf, u64) {
    let src = root.join("src");
    sample(&src, mtime_secs);
    let store_path = root.join("store");
    let store = Store::open(&store_path).expect("store");
    let tree = Tree {
        object: store.put_tree(&src).expect("put").into(),
        entries: Vec::new(),
    };
    let made = fs_vfat(
        &store_path.display().to_string(),
        &tool("mformat"),
        &tool("mcopy"),
        tree,
        size,
        fat_size,
        &Ctx::new(),
    )
    .expect("builds");
    (made.object.path_in(&store).expect("in store"), made.size.0)
}

fn fsck(image: &Path) -> std::process::Output {
    Command::new(tool("fsck.vfat"))
        .arg("-n")
        .arg(image)
        .output()
        .expect("fsck.vfat")
}

#[test]
fn the_same_tree_gives_the_same_image_and_fsck_is_clean() {
    if !available() {
        return;
    }
    let a = tempfile::tempdir().expect("tempdir");
    let b = tempfile::tempdir().expect("tempdir");
    // The second tree's entries carry other times: the image must not.
    let (path, size) = image(a.path(), 1_700_000_000, 0, 0);
    let (other, _) = image(b.path(), 1_000_000_000, 0, 0);
    let digest = file_digest(&path).expect("digest");
    println!("auto size {size} bytes, store a: {digest}");
    assert_eq!(digest, file_digest(&other).expect("digest"));

    let out = fsck(&path);
    println!(
        "fsck.vfat -n: exit {}\n{}",
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The contents are there: read one file back out of the image.
    let back = a.path().join("config.read");
    let status = Command::new(tool("mcopy"))
        .arg("-i")
        .arg(&path)
        .arg("::config.txt")
        .arg(&back)
        .status()
        .expect("mcopy");
    assert!(status.success());
    assert_eq!(
        std::fs::read_to_string(&back).expect("read"),
        "dtoverlay=example\n"
    );
}

#[test]
fn a_size_from_the_content_is_whole_mebibytes_and_a_fixed_size_is_kept() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let (_, auto) = image(dir.path(), 1_700_000_000, 0, 0);
    assert_eq!(auto % (1024 * 1024), 0, "auto size {auto}");
    let fixed = 8 * 1024 * 1024;
    let other = tempfile::tempdir().expect("tempdir");
    let (path, size) = image(other.path(), 1_700_000_000, fixed, 0);
    assert_eq!(size, fixed);
    assert!(fsck(&path).status.success());
}

#[test]
fn an_asked_fat_type_is_made_or_refused() {
    if !available() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let store_path = dir.path().join("store");
    let src = dir.path().join("src");
    sample(&src, 1_700_000_000);
    let store = Store::open(&store_path).expect("store");
    let tree = || Tree {
        object: store.put_tree(&src).expect("put").into(),
        entries: Vec::new(),
    };
    let call = |fat_size: u32, size: u64| {
        fs_vfat(
            &store_path.display().to_string(),
            &tool("mformat"),
            &tool("mcopy"),
            tree(),
            size,
            fat_size,
            &Ctx::new(),
        )
    };
    let error = call(7, 8 * 1024 * 1024).expect_err("not a FAT type");
    assert!(error.to_string().contains("fat_size 7"), "{error}");

    // FAT12 below 16 MiB is what mformat makes with no size asked.
    let made = call(12, 8 * 1024 * 1024).expect("FAT12");
    let path = made.object.path_in(&store).expect("in store");
    assert!(fsck(&path).status.success());

    // FAT32 is asked with -F, which makes it at any size.
    let made = call(32, 8 * 1024 * 1024).expect("FAT32");
    assert!(
        fsck(&made.object.path_in(&store).expect("in store"))
            .status
            .success()
    );

    // From 16 MiB mformat makes FAT16, so FAT12 asked there is refused.
    let error = call(12, 16 * 1024 * 1024).expect_err("FAT12 at 16 MiB");
    assert!(
        error.to_string().contains("asks for fat_size = 12"),
        "{error}"
    );
}
