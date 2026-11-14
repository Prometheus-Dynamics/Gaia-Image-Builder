//! A filesystem image with a dm-verity tree appended and no superblock: the
//! same image gives the same root hash in two stores, `veritysetup verify`
//! accepts the appended tree at the table's hash start, a changed byte is
//! refused, the image size follows the tree size rule, and fixed salts give
//! fixed roots. Without veritysetup the tests say SKIPPED, unless
//! `GAIA_REQUIRE_TOOLS=1`, when they fail. `GAIA_VERITYSETUP` names a
//! veritysetup to test with when it is not in the usual places.

use gaia_step::Ctx;
use gaia_store::{Store, file_digest};
use gaia_types::{Image, ImageKind, Size};
use gaia_verity::{BLOCK, format_args, verity};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
        Some(&tool.to_string()),
        None,
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

fn veritysetup(tool: &str, args: &[&str]) -> Output {
    Command::new(tool).args(args).output().expect("veritysetup")
}

/// `veritysetup verify` on an appended tree: the image is both the data and
/// the hash device, the tree starts at `hash_offset`, and the salt is the
/// one the table records.
fn verify_appended(tool: &str, image: &Path, built: &Built, hash_offset: u64) -> Output {
    let name = image.to_string_lossy().into_owned();
    let salt = format!("--salt={}", built.field("salt"));
    let blocks = format!("--data-blocks={}", built.field("data_blocks"));
    let offset = format!("--hash-offset={hash_offset}");
    veritysetup(
        tool,
        &[
            "verify",
            "--no-superblock",
            &blocks,
            "--hash=sha256",
            "--data-block-size=4096",
            "--hash-block-size=4096",
            &salt,
            &offset,
            &name,
            &name,
            &built.root_hash(),
        ],
    )
}

/// Formats `data` (`blocks` blocks) into the separate `tree` file with a
/// given salt. Returns the root hash veritysetup printed and the tree's size.
fn format_given_salt(
    tool: &str,
    data: &Path,
    tree: &Path,
    blocks: u64,
    salt: &str,
) -> (String, u64) {
    let args = format_args(data, tree, blocks, salt);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = veritysetup(tool, &refs);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = String::from_utf8_lossy(&out.stdout).into_owned();
    println!("{report}");
    let root = report
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.trim() == "Root hash")
                .map(|(_, value)| value.trim().to_string())
        })
        .expect("a Root hash line");
    let tree_bytes = std::fs::metadata(tree).map(|m| m.len()).unwrap_or(0);
    (root, tree_bytes)
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
    let offset: u64 = built.hash_offset().parse().expect("offset");
    let tree = built.size - offset;

    // 10 000 bytes pad to 3 blocks: the data is 12 288 bytes and one hash
    // block follows it.
    assert_eq!(offset, 3 * BLOCK);
    assert_eq!(tree, BLOCK);
    assert_eq!(built.field("tree_bytes"), tree.to_string());
    assert_eq!(built.field("image_bytes"), built.size.to_string());

    let ok = verify_appended(&tool, &built.image, &built, offset);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );

    // The data is the input, zero-padded to a block.
    let out = std::fs::read(&built.image).expect("read");
    assert_eq!(out.len() as u64, built.size);
    assert_eq!(&out[..bytes.len()], &bytes[..]);
    assert!(out[bytes.len()..offset as usize].iter().all(|b| *b == 0));

    let tampered = dir.path().join("tampered.img");
    let mut damaged = out;
    damaged[100] ^= 0x40;
    std::fs::write(&tampered, damaged).expect("write");
    let bad = verify_appended(&tool, &tampered, &built, offset);
    assert!(!bad.status.success(), "a changed byte verified");
}

#[test]
fn the_table_points_at_the_tree_on_the_device() {
    let Some(tool) = available() else { return };
    let dir = tempfile::tempdir().expect("tempdir");
    let built = build(
        dir.path(),
        &image_bytes(3 * 4096 + 17, 5),
        "/dev/vda3",
        &tool,
    );
    // Four data blocks, the 17 bytes padded up; the tree is one block after.
    assert_eq!(built.field("data_blocks"), "4");
    assert_eq!(built.hash_offset(), "16384");
    assert_eq!(built.field("tree_bytes"), "4096");
    assert_eq!(built.field("image_bytes"), "20480");
    assert_eq!(built.size, 20480);
    // With no superblock the tree's first block is the data's length in
    // blocks: the table's hash start is 4, not 5.
    assert_eq!(built.field("hash_start_block"), "4");
    assert_eq!(
        built.field("table"),
        format!(
            "0 32 verity 1 /dev/vda3 /dev/vda3 4096 4096 4 4 sha256 {} {}",
            built.root_hash(),
            built.field("salt")
        )
    );
    // The table's own hash start, in bytes, is where veritysetup finds the tree.
    let start: u64 = built.field("hash_start_block").parse().expect("start");
    let check = verify_appended(&tool, &built.image, &built, start * BLOCK);
    assert!(
        check.status.success(),
        "the table's hash start does not find the tree: {}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn the_size_rule_holds() {
    let Some(tool) = available() else { return };
    // Data blocks and the tree veritysetup writes for them, in bytes.
    let cases: [(u64, u64); 5] = [
        (1, 0),
        (2, 4096),
        (128, 4096),
        (129, 12288),
        (16384, 528384),
    ];
    for (blocks, tree) in cases {
        let dir = tempfile::tempdir().expect("tempdir");
        let built = build(
            dir.path(),
            &image_bytes((blocks * BLOCK) as usize, 11),
            "/dev/vda3",
            &tool,
        );
        let hash_offset = blocks * BLOCK;
        assert_eq!(
            built.hash_offset(),
            hash_offset.to_string(),
            "{blocks} blocks"
        );
        assert_eq!(
            built.field("tree_bytes"),
            tree.to_string(),
            "{blocks} blocks"
        );
        assert_eq!(built.size, hash_offset + tree, "{blocks} blocks");
        assert_eq!(
            std::fs::metadata(&built.image).expect("image").len(),
            hash_offset + tree,
            "{blocks} blocks"
        );
        println!(
            "{blocks} data blocks: tree {tree} bytes, image {}",
            built.size
        );
    }
}

/// The fixed-salt vectors. Data block `i` is filled with the byte `i mod 251`.
/// The salt is the bytes 0x00..0x1f.
#[test]
fn fixed_salt_vectors_have_fixed_roots_and_tree_sizes() {
    let Some(tool) = available() else { return };
    let salt = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    let vectors: [(u64, &str, u64); 3] = [
        (
            2,
            "bd780675f50010ed023fd1f07e98f029a6aaf25a8f6ee33ec94b4b4e847e4b2e",
            4096,
        ),
        (
            129,
            "6093a2333523050b628581510028976d3d3e9c62458642a83727e6df641397a4",
            12288,
        ),
        (
            38400,
            "a73f8f9623fc3e3298871b6ef9de8ae4ba8867f23f5d8a92b87d97b0b5ddfa18",
            1245184,
        ),
    ];
    for (blocks, root, tree) in vectors {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data.img");
        let mut writer = std::io::BufWriter::new(std::fs::File::create(&data).expect("create"));
        for i in 0..blocks {
            writer
                .write_all(&[(i % 251) as u8; BLOCK as usize])
                .expect("write");
        }
        writer.flush().expect("flush");
        drop(writer);
        let tree_path = dir.path().join("tree.bin");
        let (got_root, got_tree) = format_given_salt(&tool, &data, &tree_path, blocks, salt);
        assert_eq!(got_tree, tree, "{blocks} blocks: tree bytes");
        assert_eq!(got_root, root, "{blocks} blocks: root hash");
    }
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
        Some(&"/nonexistent/veritysetup".to_string()),
        None,
        image(),
        "/dev/vda3",
        &Ctx::new(),
    )
    .expect_err("a missing tool");
    assert!(format!("{error:?}").contains("veritysetup"), "{error:?}");
    let error = verity(
        &root,
        Some(&"/nonexistent/veritysetup".to_string()),
        None,
        image(),
        "two words",
        &Ctx::new(),
    )
    .expect_err("a device with a space");
    assert!(format!("{error:?}").contains("device"), "{error:?}");
}

/// An image with an OS build is made with that build's own veritysetup:
/// host tools that lack it are refused with the line that adds it, and a
/// tool of this machine, though given, is not used.
#[test]
fn the_images_own_host_tools_are_used_and_nothing_else() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store_path = dir.path().join("store");
    let store = Store::open(&store_path).expect("store");
    let file = dir.path().join("fs.img");
    std::fs::write(&file, vec![7u8; 8192]).expect("write");
    let image = || Image {
        kind: ImageKind::Filesystem,
        object: store.put_file(&file).expect("put").into(),
        size: gaia_types::Size(8192),
        partitions: Vec::new(),
    };
    // Host tools with no veritysetup in them.
    let seed = dir.path().join("seed");
    std::fs::create_dir_all(seed.join("host/bin")).expect("mkdir");
    std::fs::write(seed.join("host/bin/dtc"), b"").expect("write");
    let tools = gaia_types::Tree {
        object: store.put_tree(&seed).expect("put").into(),
        entries: Vec::new(),
    };
    let given = "/usr/sbin/veritysetup".to_string();
    let why = verity(
        &store_path.display().to_string(),
        Some(&given),
        Some(&tools),
        image(),
        "/dev/vda3",
        &Ctx::new(),
    )
    .expect_err("no veritysetup in the host tools")
    .to_string();
    assert!(
        why.contains(
            "the image's host tools have no veritysetup: add `BR2_PACKAGE_HOST_CRYPTSETUP=y`"
        ),
        "{why}"
    );
    // Neither host tools nor a tool given.
    let why = verity(
        &store_path.display().to_string(),
        None,
        None,
        image(),
        "/dev/vda3",
        &Ctx::new(),
    )
    .expect_err("no tool at all")
    .to_string();
    assert!(
        why.contains("no veritysetup: the image has no OS build"),
        "{why}"
    );
}
