//! A filesystem image with a dm-verity tree appended: the same image gives
//! the same root hash in two stores, `veritysetup verify` accepts the tree,
//! a changed byte is refused, and the table artifact matches the superblock.
//! Without veritysetup the tests say SKIPPED, unless `GAIA_REQUIRE_TOOLS=1`,
//! when they fail. `GAIA_VERITYSETUP` names a veritysetup to test with
//! when it is not in the usual places.

use gaia_step::Ctx;
use gaia_store::{Store, file_digest};
use gaia_types::{Image, ImageKind, Size};
use gaia_verity::verity;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The veritysetup to test with: `GAIA_VERITYSETUP`, or one in the usual
/// places.
fn tool() -> Option<String> {
    if let Some(path) = std::env::var_os("GAIA_VERITYSETUP") {
        return Some(path.to_string_lossy().into_owned());
    }
    ["/usr/bin", "/usr/sbin", "/sbin"]
        .iter()
        .map(|dir| format!("{dir}/veritysetup"))
        .find(|p| Path::new(p).exists())
}

fn available() -> Option<String> {
    let found = tool();
    if found.is_none() {
        assert!(
            std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
            "GAIA_REQUIRE_TOOLS is set, and veritysetup is missing"
        );
        println!("SKIPPED: veritysetup is not installed");
    }
    found
}

/// Deterministic bytes that are not a whole number of blocks.
fn image_bytes(len: usize, seed: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 7 + seed) % 251) as u8).collect()
}

/// A built image: its file, its size, and its table artifact's fields.
struct Built {
    image: PathBuf,
    size: u64,
    table: String,
}

impl Built {
    /// The value of one table field (`root_hash`, `salt`, ...).
    fn field(&self, key: &str) -> String {
        self.table
            .lines()
            .find_map(|line| line.strip_prefix(key).and_then(|v| v.strip_prefix(' ')))
            .unwrap_or_else(|| panic!("no {key} in the table"))
            .to_string()
    }

    fn root_hash(&self) -> String {
        self.field("root_hash")
    }

    fn hash_offset(&self) -> String {
        self.field("hash_offset")
    }
}

/// Puts `bytes` into a fresh store under `root` and builds its verity tree.
fn build(root: &Path, bytes: &[u8], device: &str, tool: &str) -> Built {
    let input = root.join("input.img");
    std::fs::write(&input, bytes).expect("write");
    let store_path = root.join("store");
    let store = Store::open(&store_path).expect("store");
    let object = store.put_file(&input).expect("put");
    let image = Image {
        kind: ImageKind::Filesystem,
        object: object.into(),
        size: Size(bytes.len() as u64),
        partitions: Vec::new(),
    };
    let (out, table) = verity(
        &store_path.display().to_string(),
        tool,
        image,
        device,
        &Ctx::new(),
    )
    .expect("builds");
    Built {
        image: out.object.path_in(&store).expect("in store"),
        size: out.size.0,
        table: std::fs::read_to_string(table.object.path_in(&store).expect("in store"))
            .expect("table"),
    }
}

fn veritysetup(tool: &str, args: &[&str]) -> std::process::Output {
    Command::new(tool).args(args).output().expect("veritysetup")
}

#[test]
fn the_same_image_gives_the_same_root_hash() {
    let Some(tool) = available() else { return };
    let bytes = image_bytes(10_000, 3);
    let (a, b) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let first = build(a.path(), &bytes, "/dev/vda3", &tool);
    let second = build(b.path(), &bytes, "/dev/vda3", &tool);
    println!("root hash: {}", first.root_hash());
    assert_eq!(first.root_hash(), second.root_hash());
    assert_eq!(first.table, second.table);
    assert_eq!(
        file_digest(&first.image).expect("digest"),
        file_digest(&second.image).expect("digest")
    );

    let other = build(
        tempfile::tempdir().expect("tempdir").path(),
        &image_bytes(10_000, 4),
        "/dev/vda3",
        &tool,
    );
    assert_ne!(first.root_hash(), other.root_hash());
}

#[test]
fn veritysetup_accepts_the_appended_tree_and_refuses_a_changed_byte() {
    let Some(tool) = available() else { return };
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = image_bytes(10_000, 9);
    let built = build(dir.path(), &bytes, "/dev/vda3", &tool);
    let offset = built.hash_offset();
    let flag = format!("--hash-offset={offset}");

    let ok = veritysetup(
        &tool,
        &[
            "verify",
            &built.image.to_string_lossy(),
            &built.image.to_string_lossy(),
            &built.root_hash(),
            &flag,
        ],
    );
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );

    // The data is the input, zero-padded to a block.
    let out = std::fs::read(&built.image).expect("read");
    assert_eq!(&out[..bytes.len()], &bytes[..]);
    assert!(
        out[bytes.len()..offset.parse::<usize>().expect("offset")]
            .iter()
            .all(|b| *b == 0)
    );
    assert!(built.size > offset.parse::<u64>().expect("offset"));

    let tampered = dir.path().join("tampered.img");
    std::fs::copy(&built.image, &tampered).expect("copy");
    let mut damaged = out;
    damaged[100] ^= 0x40;
    std::fs::write(&tampered, damaged).expect("write");
    let bad = veritysetup(
        &tool,
        &[
            "verify",
            &tampered.to_string_lossy(),
            &tampered.to_string_lossy(),
            &built.root_hash(),
            &flag,
        ],
    );
    assert!(!bad.status.success(), "a changed byte verified");
}

#[test]
fn the_table_matches_the_superblock_on_the_device() {
    let Some(tool) = available() else { return };
    let dir = tempfile::tempdir().expect("tempdir");
    let built = build(
        dir.path(),
        &image_bytes(3 * 4096 + 17, 5),
        "/dev/vda3",
        &tool,
    );
    let dump = veritysetup(
        &tool,
        &[
            "dump",
            &built.image.to_string_lossy(),
            &format!("--hash-offset={}", built.hash_offset()),
        ],
    );
    assert!(dump.status.success());
    let text = String::from_utf8_lossy(&dump.stdout).into_owned();
    println!("{text}");
    let line = |label: &str| {
        text.lines()
            .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim() == label))
            .map(|(_, v)| v.split_whitespace().next().unwrap_or_default().to_string())
            .unwrap_or_default()
    };
    assert_eq!(line("Salt"), built.field("salt"));
    assert_eq!(line("UUID"), built.field("uuid"));
    assert_eq!(line("Data blocks"), built.field("data_blocks"));
    assert_eq!(line("Data block size"), "4096");
    assert_eq!(line("Hash block size"), "4096");
    // Four data blocks, the 17 bytes padded up; the table line covers them.
    assert_eq!(built.field("data_blocks"), "4");
    assert_eq!(
        built.field("table"),
        format!(
            "0 32 verity 1 /dev/vda3 /dev/vda3 4096 4096 4 {} sha256 {} {}",
            built.field("hash_start_block"),
            built.root_hash(),
            built.field("salt")
        )
    );
}

#[test]
fn a_bad_device_or_a_missing_tool_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store_path = dir.path().join("store");
    let input = dir.path().join("in.img");
    std::fs::write(&input, image_bytes(4096, 1)).expect("write");
    let store = Store::open(&store_path).expect("store");
    let object = store.put_file(&input).expect("put");
    let image = || Image {
        kind: ImageKind::Filesystem,
        object: object.into(),
        size: Size(4096),
        partitions: Vec::new(),
    };
    let root = store_path.display().to_string();
    let error = verity(
        &root,
        "/nonexistent/veritysetup",
        image(),
        "/dev/vda3",
        &Ctx::new(),
    )
    .expect_err("a missing tool");
    assert!(format!("{error:?}").contains("veritysetup"), "{error:?}");
    let error = verity(
        &root,
        "/nonexistent/veritysetup",
        image(),
        "two words",
        &Ctx::new(),
    )
    .expect_err("a device with a space");
    assert!(format!("{error:?}").contains("device"), "{error:?}");
}
