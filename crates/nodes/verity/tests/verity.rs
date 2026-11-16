//! A filesystem image with a dm-verity tree appended and no superblock.
//!
//! The tree is computed in this process, and these tests hold it to the
//! bytes veritysetup writes. Three kinds of evidence:
//!
//! - Golden fixtures (`tests/golden/verity-2.8.7.txt`): roots, tree sizes,
//!   tree digests and the head of the tree, as veritysetup 2.8.7 wrote them.
//!   These always run, so the format is pinned without veritysetup.
//! - Baseline digests: the image and table digests the step produced before
//!   it computed the tree itself, for five inputs. The new step must give
//!   the same bytes.
//! - The oracle: `veritysetup` itself, where it is installed (`GAIA_VERITYSETUP`
//!   names it, else the usual places). Without it those tests print SKIPPED,
//!   unless `GAIA_REQUIRE_TOOLS` is set, when they fail.

use gaia_step::Ctx;
use gaia_store::{Store, file_digest};
use gaia_types::{Image, ImageKind, Size};
use gaia_verity::{BLOCK, hash_tree, verity};
use sha2::{Digest as _, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BLOCK64: u64 = BLOCK;

/// The veritysetup to test with, if there is one.
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

fn sha256_hex(bytes: &[u8]) -> String {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    gaia_verity::hex(&digest)
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
fn build(root: &Path, bytes: &[u8], device: &str) -> Built {
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

/// The `veritysetup format` arguments for a tree with no superblock, as the
/// step ran them before it computed the tree itself. The tree goes to `hash`,
/// a separate file, and the data is `data`.
fn format_args(data: &Path, hash: &Path, data_blocks: u64, salt: &str) -> Vec<String> {
    vec![
        "format".into(),
        "--no-superblock".into(),
        format!("--data-blocks={data_blocks}"),
        "--hash=sha256".into(),
        format!("--data-block-size={BLOCK}"),
        format!("--hash-block-size={BLOCK}"),
        format!("--salt={salt}"),
        data.display().to_string(),
        hash.display().to_string(),
    ]
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
/// given salt. Returns the root hash veritysetup printed and the tree bytes.
fn format_oracle(
    tool: &str,
    data: &Path,
    tree: &Path,
    blocks: u64,
    salt: &str,
) -> (String, Vec<u8>) {
    let _ = std::fs::remove_file(tree);
    let args = format_args(data, tree, blocks, salt);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = veritysetup(tool, &refs);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = String::from_utf8_lossy(&out.stdout).into_owned();
    let root = report
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.trim() == "Root hash")
                .map(|(_, value)| value.trim().to_string())
        })
        .expect("a Root hash line");
    let bytes = std::fs::read(tree).unwrap_or_default();
    (root, bytes)
}

/// Writes `blocks` blocks of `image_bytes` to `path`.
fn write_data(path: &Path, blocks: u64, seed: usize) {
    let mut writer = std::io::BufWriter::new(std::fs::File::create(path).expect("create"));
    writer
        .write_all(&image_bytes((blocks * BLOCK64) as usize, seed))
        .expect("write");
    writer.flush().expect("flush");
}

/// The new code's root and tree for `blocks` blocks of `image_bytes`.
fn hash_pattern(blocks: u64, seed: usize, salt: &[u8]) -> (String, Vec<u8>) {
    let data = image_bytes((blocks * BLOCK64) as usize, seed);
    let mut tree = Vec::new();
    let (root, len) = hash_tree(&mut &data[..], blocks, salt, &mut tree).expect("hashes");
    assert_eq!(len, tree.len() as u64);
    (gaia_verity::hex(&root), tree)
}

#[test]
fn the_same_image_gives_the_same_root_hash() {
    let bytes = image_bytes(10_000, 3);
    let (a, b) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let first = build(a.path(), &bytes, "/dev/vda3");
    let second = build(b.path(), &bytes, "/dev/vda3");
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
    );
    assert_ne!(first.root_hash(), other.root_hash());
}

#[test]
fn veritysetup_accepts_the_appended_tree_and_refuses_a_changed_byte() {
    let Some(tool) = available() else { return };
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = image_bytes(10_000, 9);
    let built = build(dir.path(), &bytes, "/dev/vda3");
    let offset: u64 = built.hash_offset().parse().expect("offset");
    let tree = built.size - offset;

    // 10 000 bytes pad to 3 blocks: the data is 12 288 bytes and one hash
    // block follows it.
    assert_eq!(offset, 3 * BLOCK64);
    assert_eq!(tree, BLOCK64);
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
    let dir = tempfile::tempdir().expect("tempdir");
    let built = build(dir.path(), &image_bytes(3 * 4096 + 17, 5), "/dev/vda3");
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
}

#[test]
fn the_table_hash_start_finds_the_tree_under_veritysetup() {
    let Some(tool) = available() else { return };
    let dir = tempfile::tempdir().expect("tempdir");
    let built = build(dir.path(), &image_bytes(3 * 4096 + 17, 5), "/dev/vda3");
    let start: u64 = built.field("hash_start_block").parse().expect("start");
    let check = verify_appended(&tool, &built.image, &built, start * BLOCK64);
    assert!(
        check.status.success(),
        "the table's hash start does not find the tree: {}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn the_size_rule_holds() {
    // Data blocks and the tree for them, in bytes.
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
            &image_bytes((blocks * BLOCK64) as usize, 11),
            "/dev/vda3",
        );
        let hash_offset = blocks * BLOCK64;
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
    }
}

/// The fixed-salt vectors. Data block `i` is filled with the byte `i mod 251`.
/// The salt is the bytes 0x00..0x1f. These are the roots veritysetup gave
/// for the same data, before the tree was computed here.
#[test]
fn fixed_salt_vectors_have_fixed_roots_and_tree_sizes() {
    let salt: Vec<u8> = (0..32).collect();
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
        let data: Vec<u8> = (0..blocks)
            .flat_map(|i| std::iter::repeat_n((i % 251) as u8, BLOCK as usize))
            .collect();
        let mut out = Vec::new();
        let (got_root, len) = hash_tree(&mut &data[..], blocks, &salt, &mut out).expect("hashes");
        assert_eq!(len, tree, "{blocks} blocks: tree bytes");
        assert_eq!(
            gaia_verity::hex(&got_root),
            root,
            "{blocks} blocks: root hash"
        );
    }
}

/// The golden fixtures: veritysetup 2.8.7's root, tree size, tree digest and
/// the first 512 bytes of the tree, for patterned data and several salts.
#[test]
fn golden_fixtures_from_veritysetup_2_8_7_hold() {
    let fixtures = include_str!("golden/verity-2.8.7.txt");
    let mut checked = 0;
    for line in fixtures.lines().filter(|line| !line.starts_with('#')) {
        // The one-block case has an empty tree, so its head is empty text.
        let words: Vec<&str> = line.split(' ').collect();
        let field = |key: &str| {
            words
                .windows(2)
                .find(|pair| pair[0] == key)
                .map(|pair| pair[1])
                .or((key == "head512").then_some(""))
                .unwrap_or_else(|| panic!("no {key} in {line}"))
        };
        let blocks: u64 = field("blocks").parse().expect("blocks");
        let seed: usize = field("seed").parse().expect("seed");
        let salt = match field("salt") {
            "-" => Vec::new(),
            hex => (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("salt hex"))
                .collect(),
        };
        let (root, tree) = hash_pattern(blocks, seed, &salt);
        assert_eq!(root, field("root"), "{blocks} blocks: root");
        assert_eq!(
            tree.len().to_string(),
            field("tree_bytes"),
            "{blocks} blocks: tree bytes"
        );
        assert_eq!(
            sha256_hex(&tree),
            field("tree_sha256"),
            "{blocks} blocks: tree digest"
        );
        let head: String = tree
            .iter()
            .take(512)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(head, field("head512"), "{blocks} blocks: first 512 bytes");
        checked += 1;
    }
    assert_eq!(checked, 8, "the fixture has its eight cases");
}

/// The oracle: for each count and salt, the tree and the root veritysetup
/// writes are the new code's, byte for byte. The salts are a 32-byte digest
/// (what the step passes) and the empty salt (`--salt=-`).
#[test]
fn the_tree_matches_veritysetup_for_many_sizes_and_salts() {
    let Some(tool) = available() else { return };
    let dir = tempfile::tempdir().expect("tempdir");
    let data = dir.path().join("data.img");
    let tree = dir.path().join("tree.bin");
    let digest_salt: Vec<u8> = (0..32_u8)
        .map(|b| b.wrapping_mul(37).wrapping_add(1))
        .collect();
    let salts: [(Vec<u8>, String); 2] = [
        (digest_salt.clone(), gaia_verity::hex(&digest_salt)),
        (Vec::new(), "-".to_string()),
    ];
    for blocks in [1_u64, 2, 127, 128, 129, 300, 16_384, 16_385] {
        write_data(&data, blocks, 21);
        for (salt_bytes, salt_arg) in &salts {
            let (oracle_root, oracle_tree) = format_oracle(&tool, &data, &tree, blocks, salt_arg);
            let mut ours = Vec::new();
            let (root, len) = hash_tree(
                &mut std::fs::File::open(&data).expect("open"),
                blocks,
                salt_bytes,
                &mut ours,
            )
            .expect("hashes");
            assert_eq!(
                len,
                oracle_tree.len() as u64,
                "{blocks} blocks, salt {salt_arg}: tree bytes"
            );
            assert_eq!(
                ours, oracle_tree,
                "{blocks} blocks, salt {salt_arg}: tree bytes differ"
            );
            assert_eq!(
                gaia_verity::hex(&root),
                oracle_root,
                "{blocks} blocks, salt {salt_arg}: root"
            );
        }
    }
}

/// The UUID the step passes to veritysetup does not change its bytes: with no
/// superblock the UUID is not written anywhere in the hash area or the root.
#[test]
fn veritysetup_gives_the_same_tree_with_the_uuid_the_step_passes() {
    let Some(tool) = available() else { return };
    let dir = tempfile::tempdir().expect("tempdir");
    let data = dir.path().join("data.img");
    let tree = dir.path().join("tree.bin");
    write_data(&data, 129, 2);
    let salt = "ab".repeat(32);
    let (_, plain) = format_oracle(&tool, &data, &tree, 129, &salt);
    let args = format_args(&data, &tree, 129, &salt);
    let mut with_uuid: Vec<&str> = args.iter().map(String::as_str).collect();
    with_uuid.push("--uuid=3409f8c2-8488-4f52-af8c-c67d5224f326");
    let _ = std::fs::remove_file(&tree);
    let out = veritysetup(&tool, &with_uuid);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let uuid_tree = std::fs::read(&tree).expect("tree");
    assert_eq!(uuid_tree, plain);
    assert!(String::from_utf8_lossy(&out.stdout).contains("Root hash:"));
}

/// The step's bytes before it computed the tree itself: the image and table
/// digests for five inputs, as the veritysetup-based step wrote them. The
/// inputs are generated, not stored.
#[test]
fn the_step_gives_the_bytes_the_veritysetup_step_gave() {
    let baseline: [(usize, usize, &str, &str); 5] = [
        (
            1,
            3,
            "0e2e1ff956a39192cbb68d2212288fe75b32733ab0c442b9f0471e254a0382a2",
            "4b0d13377be489c695eaf412fd1f88bc1e3afaa55e8680deaf85a29fa4f6c8c8",
        ),
        (
            4096,
            3,
            "0d356260eaf09e3b3dc81a65b2ad2399aa7c4921c0274bd2cbb54c2a21c46e3b",
            "725e81c7af05bac7192f21b1984bb84c59697c03692ee6b187a8b90ecbeefb54",
        ),
        (
            10000,
            9,
            "bf13465df4764be04473c50f13921d8b7eacb12cd3dfae8860446dba40c9ffc1",
            "40a21319c5fe06ad6e8ee91411cfc6b7818b310a89eaed7c8e3afc5246e35fc3",
        ),
        (
            4096 * 129 + 7,
            2,
            "bf2d6273adce0ddc0f61d27d279dc5b351082c3bdc515879560637613b3afb80",
            "f3a5a7d1f562e4398f69a9ff81731e8d1293e2376f616140738f6f55df7d3f2c",
        ),
        (
            4096 * 16385,
            5,
            "c31f84e8068d67781fc57d21c6e712801bf0a51e51d34cf5a432833da3b85a57",
            "bdb22d0a088704f1dab2551b3af87cdb897abe821bf989be0343eb6e06c0f4f5",
        ),
    ];
    for (len, seed, image_digest, table_digest) in baseline {
        let dir = tempfile::tempdir().expect("tempdir");
        let built = build(dir.path(), &image_bytes(len, seed), "/dev/vda3");
        assert_eq!(
            file_digest(&built.image).expect("digest").hex(),
            image_digest,
            "image of {len} bytes"
        );
        let table = dir.path().join("table.txt");
        std::fs::write(&table, &built.table).expect("write");
        assert_eq!(
            file_digest(&table).expect("digest").hex(),
            table_digest,
            "table of {len} bytes"
        );
    }
}

#[test]
fn a_bad_device_is_an_error() {
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
    let error =
        verity(&root, image(), "two words", &Ctx::new()).expect_err("a device with a space");
    assert!(format!("{error:?}").contains("device"), "{error:?}");
    let error = verity(&root, image(), "", &Ctx::new()).expect_err("no device");
    assert!(format!("{error:?}").contains("device"), "{error:?}");
}

/// Timing: hashing a 300 MB image, and veritysetup on the same file. Run
/// with `--ignored --nocapture`; not part of the normal run.
#[test]
#[ignore]
fn timing_300_mb_against_veritysetup() {
    let dir = tempfile::tempdir().expect("tempdir");
    let data = dir.path().join("data.img");
    let blocks = 300 * 1024 * 1024 / BLOCK64;
    write_data(&data, blocks, 7);
    let salt: Vec<u8> = (0..32).collect();
    let started = std::time::Instant::now();
    let mut tree = Vec::new();
    let (root, _) = hash_tree(
        &mut std::io::BufReader::new(std::fs::File::open(&data).expect("open")),
        blocks,
        &salt,
        &mut tree,
    )
    .expect("hashes");
    println!(
        "hash_tree 300 MB: {:?}, root {}",
        started.elapsed(),
        gaia_verity::hex(&root)
    );
    if let Some(tool) = tool() {
        let tree_path = dir.path().join("tree.bin");
        let started = std::time::Instant::now();
        let (oracle_root, oracle_tree) =
            format_oracle(&tool, &data, &tree_path, blocks, &gaia_verity::hex(&salt));
        println!(
            "veritysetup 300 MB: {:?}, root {oracle_root}",
            started.elapsed()
        );
        assert_eq!(oracle_tree, tree);
    }
}
