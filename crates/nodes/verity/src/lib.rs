//! `gaia.verity`: a filesystem image with a dm-verity hash tree appended,
//! with no superblock.
//!
//! The image is copied and zero-padded to whole 4 KiB data blocks.
//! `veritysetup format --no-superblock` writes the hash tree to a separate
//! file, and that file's bytes are appended to the data. The output is the
//! data followed by the tree, nothing else. `hash_offset` is the data's
//! length (`data_blocks * 4096`), and the tree's length is the size of the
//! file veritysetup wrote, so a one-block image has no tree at all. The
//! dm table's hash start is the tree's first block, `hash_offset / 4096`.
//!
//! The salt is the image's digest in hex, so the same image gives the same
//! root hash on every host. The UUID is derived from the same digest and
//! passed to veritysetup. The format is kernel type 1 with sha256 and 4 KiB
//! data and hash blocks.
//!
//! Two outputs. `image` is the padded data with the tree appended, the same
//! file the device holds. `table` is a text artifact with the root hash,
//! salt, UUID, data blocks, offsets and tree length, and the dm-verity table
//! line for `device`, which is the device the image is written to (the tree
//! is on that same device).
//!
//! `veritysetup` is pinned by path; its version is in the key, with the
//! image's digest, so a changed image or tool remakes the tree. The key
//! version moved to 2 when the layout changed, so outputs of the old
//! superblock form are not reused.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Tool};
use gaia_store::Digest;
use gaia_types::{Artifact, ArtifactKind, Image, ImageKind, Size};
use std::path::Path;

const NAME: &str = "gaia.verity";
/// The data and hash block size, the same for both.
pub const BLOCK: u64 = 4096;
/// Hash type 1: the kernel's format.
const HASH_TYPE_KERNEL: u32 = 1;
/// The table line's version field.
const TABLE_VERSION: u32 = 1;
/// The table file's name in the output directory.
const TABLE_FILE: &str = "verity.txt";
/// The image file's name in the output directory.
const IMAGE_FILE: &str = "image.img";
/// The separate file veritysetup writes the tree to, before it is appended.
const TREE_FILE: &str = "tree.bin";

/// The image with a hash tree appended, and the artifact that says how to open it.
#[node(
    id = "gaia.verity",
    inputs(store, veritysetup, tools, image, device),
    outputs(image, table)
)]
pub fn verity(
    store: &str,
    veritysetup: Option<&String>,
    tools: Option<&gaia_types::Tree>,
    image: Image,
    device: &str,
    cx: &Ctx,
) -> Result<(Image, Artifact), StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |e: String| StepError::body(NAME, e);
    // The image's own build's veritysetup when it has one, never this
    // machine's.
    let program = gaia_step::image_tool(
        &store,
        tools.and_then(|tree| tree.object.object_ref()).as_ref(),
        veritysetup.map(String::as_str),
        "veritysetup",
        "BR2_PACKAGE_HOST_CRYPTSETUP=y",
    )
    .map_err(fail)?;
    let program = program.display().to_string();
    let veritysetup: &str = &program;
    if device.is_empty() || device.chars().any(char::is_whitespace) {
        return Err(fail(format!(
            "device {device:?} must be a non-empty path without spaces"
        )));
    }
    let object = image
        .object
        .object_ref()
        .ok_or_else(|| fail("an image that is not in the store".into()))?;
    let src = store.path(&object);
    let len = std::fs::metadata(&src)
        .map_err(|e| fail(e.to_string()))?
        .len();
    if len == 0 {
        return Err(fail("an empty image has no data to verify".into()));
    }
    let version = std::process::Command::new(veritysetup)
        .arg("--version")
        .env_clear()
        .output()
        .map_err(|e| fail(format!("{veritysetup} --version: {e}")))?;
    let version = [version.stdout, version.stderr].concat();
    let key = KeyBuilder::new(NAME, 2)
        .config("veritysetup", &version)
        .config("device", device)
        .config("block", BLOCK.to_string())
        .input("image", object.digest)
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[
            ("image", Path::new(IMAGE_FILE)),
            ("table", Path::new(TABLE_FILE)),
        ],
        |work| {
            let data = work.dir().join(IMAGE_FILE);
            std::fs::copy(&src, &data).map_err(|e| fail(format!("copy: {e}")))?;
            let padded = padded_len(len, BLOCK);
            std::fs::OpenOptions::new()
                .write(true)
                .open(&data)
                .and_then(|file| file.set_len(padded))
                .map_err(|e| fail(format!("pad to {padded} bytes: {e}")))?;
            let data_blocks = padded / BLOCK;
            let hash_offset = data_blocks * BLOCK;
            let salt = object.digest.hex();
            let uuid = uuid_of(&object.digest);
            let tree = work.dir().join(TREE_FILE);
            let out = work.run(
                &Tool::new(veritysetup)
                    .args(format_args(&data, &tree, data_blocks, &salt))
                    .arg(format!("--uuid={uuid}")),
            )?;
            let report = out.stdout_tail.join("\n");
            let root_hash = report_value(&report, "Root hash").map_err(fail)?;
            if !is_hex_of_len(&root_hash, 64) {
                return Err(fail(format!(
                    "veritysetup reported root hash {root_hash:?}, not a sha256 hex digest"
                )));
            }
            let blocks = report_value(&report, "Data blocks")
                .map_err(fail)?
                .parse::<u64>()
                .map_err(|e| fail(format!("data blocks: {e}")))?;
            if blocks != data_blocks {
                return Err(fail(format!(
                    "veritysetup covered {blocks} data blocks, expected {data_blocks}"
                )));
            }
            let tree_bytes = tree_len(&tree).map_err(fail)?;
            if tree_bytes > 0 {
                append_tree(&tree, &data).map_err(fail)?;
            }
            remove_tree(&tree).map_err(fail)?;
            let image_bytes = hash_offset + tree_bytes;
            let written = std::fs::metadata(&data)
                .map_err(|e| fail(e.to_string()))?
                .len();
            if written != image_bytes {
                return Err(fail(format!(
                    "the image is {written} bytes, expected {image_bytes} ({hash_offset} data and {tree_bytes} tree)"
                )));
            }
            let table = Table {
                root_hash,
                salt,
                uuid,
                data_blocks,
                hash_offset,
                tree_bytes,
                image_bytes,
                device: device.to_string(),
            };
            std::fs::write(work.dir().join(TABLE_FILE), table.text())
                .map_err(|e| fail(e.to_string()))
        },
    )?;
    let image_object = made.outputs["image"];
    let bytes = std::fs::metadata(store.path(&image_object))
        .map_err(|e| fail(e.to_string()))?
        .len();
    Ok((
        Image {
            kind: ImageKind::Filesystem,
            object: image_object.into(),
            size: Size(bytes),
            partitions: Vec::new(),
        },
        Artifact {
            kind: ArtifactKind::parse("verity"),
            object: made.outputs["table"].into(),
        },
    ))
}

/// What the table artifact records: enough to open the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub root_hash: String,
    pub salt: String,
    pub uuid: String,
    pub data_blocks: u64,
    /// The tree's offset in the image: the data's length.
    pub hash_offset: u64,
    /// The appended tree's length, as veritysetup wrote it.
    pub tree_bytes: u64,
    /// The whole image: the data and the tree.
    pub image_bytes: u64,
    pub device: String,
}

impl Table {
    /// The data's length in 512-byte sectors.
    pub fn sectors(&self) -> u64 {
        self.data_blocks * BLOCK / 512
    }

    /// The tree's first block on the device. With no superblock the tree
    /// starts right at `hash_offset`.
    pub fn hash_start(&self) -> u64 {
        self.hash_offset / BLOCK
    }

    /// The dm-verity table line for the device, the form `dmsetup create`
    /// takes: `0 <sectors> verity 1 <dev> <dev> <block> <block> <blocks>
    /// <hash start> sha256 <root hash> <salt>`.
    pub fn line(&self) -> String {
        format!(
            "0 {} verity {TABLE_VERSION} {dev} {dev} {BLOCK} {BLOCK} {} {} sha256 {} {}",
            self.sectors(),
            self.data_blocks,
            self.hash_start(),
            self.root_hash,
            self.salt,
            dev = self.device,
        )
    }

    /// The table file: one `key value` line per field, then the table line.
    pub fn text(&self) -> String {
        format!(
            "hash_type {HASH_TYPE_KERNEL}\nalgorithm sha256\nroot_hash {}\nsalt {}\nuuid {}\n\
             data_blocks {}\ndata_block_size {BLOCK}\nhash_block_size {BLOCK}\n\
             hash_offset {}\ntree_bytes {}\nimage_bytes {}\nhash_start_block {}\ntable {}\n",
            self.root_hash,
            self.salt,
            self.uuid,
            self.data_blocks,
            self.hash_offset,
            self.tree_bytes,
            self.image_bytes,
            self.hash_start(),
            self.line(),
        )
    }
}

/// The `veritysetup format` arguments for a tree with no superblock: the
/// tree goes to `hash`, a separate file, and the data is `data`. The node
/// runs these through its tool, and the tests run the same arguments
/// directly with a given salt.
pub fn format_args(data: &Path, hash: &Path, data_blocks: u64, salt: &str) -> Vec<String> {
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

/// The length rounded up to whole data blocks: `veritysetup` covers exactly
/// these bytes, so the image is zero-padded to them.
pub fn padded_len(len: u64, block: u64) -> u64 {
    len.div_ceil(block) * block
}

/// The length of the tree veritysetup wrote. A missing file is an empty
/// tree, which is what one data block gives.
fn tree_len(tree: &Path) -> Result<u64, String> {
    match std::fs::metadata(tree) {
        Ok(meta) => Ok(meta.len()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(format!("tree {}: {e}", tree.display())),
    }
}

/// Appends the tree's bytes to the end of the image.
fn append_tree(tree: &Path, data: &Path) -> Result<(), String> {
    let mut from = std::fs::File::open(tree).map_err(|e| format!("open tree: {e}"))?;
    let mut image = std::fs::OpenOptions::new()
        .append(true)
        .open(data)
        .map_err(|e| format!("open image: {e}"))?;
    std::io::copy(&mut from, &mut image)
        .map(|_| ())
        .map_err(|e| format!("append tree: {e}"))
}

/// Removes the separate tree file; it is not an output.
fn remove_tree(tree: &Path) -> Result<(), String> {
    match std::fs::remove_file(tree) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove tree: {e}")),
    }
}

/// The value after `label:` in `veritysetup format`'s report.
fn report_value(report: &str, label: &str) -> Result<String, String> {
    report
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == label).then(|| value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("veritysetup output has no {label:?} line"))
}

/// Exactly `len` lowercase hex digits.
fn is_hex_of_len(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// A version 4 UUID from the digest's first 16 bytes, the way the EROFS
/// node derives its own.
fn uuid_of(digest: &Digest) -> String {
    let hex = digest.hex();
    let mut bytes = [0u8; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&VERITY_NODE];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_pads_to_whole_blocks() {
        assert_eq!(padded_len(1, BLOCK), BLOCK);
        assert_eq!(padded_len(BLOCK, BLOCK), BLOCK);
        assert_eq!(padded_len(BLOCK + 1, BLOCK), 2 * BLOCK);
    }

    #[test]
    fn table_line_is_the_dm_verity_form() {
        let table = Table {
            root_hash: "ab".repeat(32),
            salt: "cd".repeat(32),
            uuid: "01234567-89ab-4cde-8f01-23456789abcd".into(),
            data_blocks: 3,
            hash_offset: 3 * BLOCK,
            tree_bytes: BLOCK,
            image_bytes: 4 * BLOCK,
            device: "/dev/vda3".into(),
        };
        assert_eq!(table.sectors(), 24);
        assert_eq!(table.hash_start(), 3);
        assert_eq!(
            table.line(),
            format!(
                "0 24 verity 1 /dev/vda3 /dev/vda3 4096 4096 3 3 sha256 {} {}",
                "ab".repeat(32),
                "cd".repeat(32)
            )
        );
        assert!(table.text().contains("hash_start_block 3\n"));
        assert!(
            table
                .text()
                .contains("tree_bytes 4096\nimage_bytes 16384\n")
        );
    }

    #[test]
    fn report_values_are_found_by_label() {
        let report =
            "VERITY header information for d.img\nData blocks:     \t2\nRoot hash:      \tabcd\n";
        assert_eq!(
            report_value(report, "Root hash").expect("root hash"),
            "abcd"
        );
        assert_eq!(report_value(report, "Data blocks").expect("blocks"), "2");
        assert!(report_value(report, "Salt").is_err());
    }

    #[test]
    fn salts_are_lowercase_hex() {
        assert!(is_hex_of_len(&"0a".repeat(32), 64));
        assert!(!is_hex_of_len(&"0A".repeat(32), 64));
        assert!(!is_hex_of_len(&"0a".repeat(31), 64));
    }

    #[test]
    fn uuids_are_version_4() {
        let uuid = uuid_of(&Digest::of_bytes(b"image"));
        assert_eq!(uuid.len(), 36);
        assert_eq!(uuid.as_bytes()[14], b'4');
        assert!(matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert_eq!(uuid, uuid_of(&Digest::of_bytes(b"image")));
    }
}
