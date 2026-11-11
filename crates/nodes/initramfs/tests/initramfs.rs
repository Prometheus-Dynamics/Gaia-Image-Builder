//! A tree as a newc initramfs: the same tree gives the same bytes in two
//! stores, the archive holds exactly the tree's paths, owners and modes
//! survive, a change to either changes the digest, and a compressed copy is
//! the same archive. `cpio` reads it back when it is installed; without it
//! that check says SKIPPED, unless `GAIA_REQUIRE_TOOLS=1`, when it fails.

use gaia_compress::compress_gzip;
use gaia_initramfs::initramfs;
use gaia_step::Ctx;
use gaia_store::{Store, file_digest};
use gaia_types::{Artifact, FileEntry, Tree};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

/// What one record of the archive holds.
#[derive(Debug, PartialEq, Eq)]
struct Record {
    name: String,
    mode: u32,
    uid: u32,
    gid: u32,
    data: Vec<u8>,
}

/// Reads every record of a newc archive, the trailer last.
fn parse_newc(bytes: &[u8]) -> Vec<Record> {
    let field = |at: usize| -> u32 {
        u32::from_str_radix(std::str::from_utf8(&bytes[at..at + 8]).expect("hex"), 16)
            .expect("hex number")
    };
    let mut records = Vec::new();
    let mut at = 0;
    loop {
        assert_eq!(&bytes[at..at + 6], b"070701", "magic at {at}");
        let mode = field(at + 14);
        let uid = field(at + 22);
        let gid = field(at + 30);
        let filesize = field(at + 54) as usize;
        let namesize = field(at + 94) as usize;
        let name_at = at + 110;
        let name =
            String::from_utf8(bytes[name_at..name_at + namesize - 1].to_vec()).expect("utf8 name");
        let data_at = (name_at + namesize).div_ceil(4) * 4;
        let data = bytes[data_at..data_at + filesize].to_vec();
        at = (data_at + filesize).div_ceil(4) * 4;
        let done = name == "TRAILER!!!";
        records.push(Record {
            name,
            mode,
            uid,
            gid,
            data,
        });
        if done {
            assert_eq!(at, bytes.len(), "no bytes after the trailer");
            return records;
        }
    }
}

fn names(records: &[Record]) -> Vec<&str> {
    records
        .iter()
        .filter(|r| r.name != "TRAILER!!!")
        .map(|r| r.name.as_str())
        .collect()
}

fn sample(dir: &Path) {
    let write = |path: &str, text: &str| {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    };
    write("bin/tool", "#!/bin/sh\necho tool\n");
    write("etc/hostname", "box\n");
    write("etc/app/config", "a = 1\n");
    write("etc-note", "not under etc/\n");
    write("var/lib/state", "0\n");
    std::os::unix::fs::symlink("tool", dir.join("bin/sh")).expect("symlink");
}

/// A store under `root` holding `src` as a tree with `entries`.
fn stored(root: &Path, src: &Path, entries: Vec<FileEntry>) -> (String, Tree) {
    let store_path = root.join("store");
    let store = Store::open(&store_path).expect("store");
    let tree = Tree {
        object: store.put_tree(src).expect("put").into(),
        entries,
    };
    (store_path.display().to_string(), tree)
}

fn entry(path: &str, mode: u32, uid: u32, gid: u32) -> FileEntry {
    FileEntry {
        path: path.into(),
        mode,
        uid,
        gid,
    }
}

fn archive_of(artifact: &Artifact, store: &str) -> Vec<u8> {
    let store = Store::open(store).expect("store");
    std::fs::read(artifact.object.path_in(&store).expect("in store")).expect("read")
}

#[test]
fn the_same_tree_gives_the_same_archive_and_its_contents_are_listed() {
    let (a, b) = (
        tempfile::tempdir().expect("a"),
        tempfile::tempdir().expect("b"),
    );
    let (src_a, src_b) = (a.path().join("src"), b.path().join("src"));
    sample(&src_a);
    sample(&src_b);
    let (store_a, tree_a) = stored(a.path(), &src_a, Vec::new());
    let (store_b, tree_b) = stored(b.path(), &src_b, Vec::new());
    let one = initramfs(&store_a, tree_a, &Ctx::new()).expect("archive");
    let two = initramfs(&store_b, tree_b, &Ctx::new()).expect("archive");
    assert_eq!(one.kind.as_str(), "initramfs");
    let (bytes_a, bytes_b) = (archive_of(&one, &store_a), archive_of(&two, &store_b));
    assert_eq!(bytes_a, bytes_b, "the same tree gives the same bytes");
    let object = one.object.object_ref().expect("ref");
    let path = Store::open(&store_a).expect("store").path(&object);
    assert_eq!(file_digest(&path).expect("digest"), object.digest);
    assert_eq!(bytes_a.len() % 4, 0, "newc pads every record to four bytes");

    // Exactly the tree's paths, component order: a parent before its children.
    let records = parse_newc(&bytes_a);
    let expected = [
        "bin",
        "bin/sh",
        "bin/tool",
        "etc",
        "etc/app",
        "etc/app/config",
        "etc/hostname",
        "etc-note",
        "var",
        "var/lib",
        "var/lib/state",
    ];
    assert_eq!(names(&records), expected);
    println!("initramfs contents: {}", expected.join(" "));
    let link = records.iter().find(|r| r.name == "bin/sh").expect("link");
    assert_eq!(link.mode & 0o170000, 0o120000, "a symlink");
    assert_eq!(link.data, b"tool");
    let tool = records.iter().find(|r| r.name == "bin/tool").expect("tool");
    assert_eq!(tool.data, b"#!/bin/sh\necho tool\n");
    assert_eq!((tool.uid, tool.gid), (0, 0), "root when no entry names it");

    cpio_lists(&bytes_a, &expected);
}

/// `cpio` lists the archive as the same names, in the same order.
fn cpio_lists(bytes: &[u8], expected: &[&str]) {
    let dir = tempfile::tempdir().expect("tempdir");
    let archive = dir.path().join("initramfs.cpio");
    std::fs::write(&archive, bytes).expect("write");
    let installed = Command::new("cpio").arg("--version").output().is_ok();
    if !installed {
        assert!(
            std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
            "GAIA_REQUIRE_TOOLS is set, and cpio is missing"
        );
        println!("SKIPPED: cpio is not installed");
        return;
    }
    let mut out = Command::new("cpio")
        .args(["--list", "--quiet"])
        .stdin(std::fs::File::open(&archive).expect("open"))
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn cpio");
    let mut text = String::new();
    out.stdout
        .take()
        .expect("stdout")
        .read_to_string(&mut text)
        .expect("read");
    assert!(
        out.wait().expect("wait").success(),
        "cpio reads the archive"
    );
    let listed: Vec<&str> = text.lines().collect();
    assert_eq!(listed, expected, "cpio lists the same paths");
}

#[test]
fn owners_and_modes_survive_and_change_the_digest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    sample(&src);
    let (store, bare) = stored(dir.path(), &src, Vec::new());
    let owned = vec![entry("etc/app/config", 0o640, 1000, 100)];
    let (_, tree) = stored(dir.path(), &src, owned);
    let plain = initramfs(&store, bare, &Ctx::new()).expect("archive");
    let with = initramfs(&store, tree, &Ctx::new()).expect("archive");
    assert_ne!(
        plain.object, with.object,
        "an owner change is a different archive"
    );
    let records = parse_newc(&archive_of(&with, &store));
    let config = records
        .iter()
        .find(|r| r.name == "etc/app/config")
        .expect("config");
    assert_eq!((config.mode, config.uid, config.gid), (0o100640, 1000, 100));
}

#[test]
fn an_entry_for_a_path_the_tree_lacks_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    sample(&src);
    let (store, tree) = stored(dir.path(), &src, vec![entry("usr/missing", 0o755, 0, 0)]);
    let error = initramfs(&store, tree, &Ctx::new()).expect_err("refused");
    assert!(error.to_string().contains("usr/missing"), "{error}");
}

#[test]
fn a_compressed_copy_is_the_same_archive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    sample(&src);
    let (store, tree) = stored(dir.path(), &src, Vec::new());
    let archive = initramfs(&store, tree, &Ctx::new()).expect("archive");
    let gz = compress_gzip(&store, archive.clone(), 9, &Ctx::new()).expect("gzip");
    assert_eq!(gz.kind.as_str(), "initramfs+gz");
    let packed = std::fs::read(
        gz.object
            .path_in(&Store::open(&store).expect("store"))
            .expect("path"),
    )
    .expect("read");
    let mut unpacked = Vec::new();
    flate2::read::GzDecoder::new(&packed[..])
        .read_to_end(&mut unpacked)
        .expect("gunzip");
    assert_eq!(unpacked, archive_of(&archive, &store));
}
