//! `gaia.verity`: a filesystem image with a dm-verity hash tree appended,
//! with no superblock.
//!
//! The image is copied and zero-padded to whole 4 KiB data blocks. The hash
//! tree is computed here (see `tree`), and its bytes are appended to the
//! data. The output is the data followed by the tree, nothing else.
//! `hash_offset` is the data's length (`data_blocks * 4096`), and the tree's
//! length is what the hashing wrote, so a one-block image has no tree at
//! all. The dm table's hash start is the tree's first block,
//! `hash_offset / 4096`.
//!
//! The salt is the image's digest, the 32 bytes written in hex in the table,
//! so the same image gives the same root hash on every host. The UUID is
//! derived from the same digest. It is recorded in the table; a tree with no
//! superblock does not contain it. The format is kernel type 1 with sha256
//! and 4 KiB data and hash blocks, the bytes `veritysetup format
//! --no-superblock` writes for the same salt; the tests pin that.
//!
//! Two outputs. `image` is the padded data with the tree appended, the same
//! file the device holds. `table` is a text artifact with the root hash,
//! salt, UUID, data blocks, offsets and tree length, and the dm-verity table
//! line for `device`, which is the device the image is written to (the tree
//! is on that same device).
//!
//! The table's text is an interface: programs that are not this builder's
//! read it (an installer's bundle writer takes the root hash, salt and
//! sizes from it, and checks them against each other). Its lines are
//! `key value`, in this order: `hash_type`, `algorithm`, `root_hash`,
//! `salt`, `uuid`, `data_blocks`, `data_block_size`, `hash_block_size`,
//! `hash_offset`, `tree_bytes`, `image_bytes`, `hash_start_block`, then
//! `table`. They hold: `hash_offset = data_blocks * data_block_size`, and
//! `image_bytes = hash_offset + tree_bytes`, the image file's length. A
//! key is not renamed or dropped, and that arithmetic not changed, without
//! its readers being told first; a test holds both.
//!
//! No tool runs. The key holds the image's digest and the device; the
//! format is fixed by the code, so a changed image remakes the tree. The key
//! version is 2 for the layout, which has not changed since version 2; the
//! key's inputs did change when `veritysetup` left them, so an entry the
//! earlier step made is not found by this one.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_store::Digest;
use gaia_types::{Artifact, ArtifactKind, Image, ImageKind, Size};
use std::io::{BufReader, BufWriter, Write as _};
use std::path::Path;

mod tree;
pub use tree::{hash_tree, hex};

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

/// The image with a hash tree appended, and the artifact that says how to open it.
#[node(
    id = "gaia.verity",
    inputs(store, image, device),
    outputs(image, table)
)]
pub fn verity(
    store: &str,
    image: Image,
    device: &str,
    cx: &Ctx,
) -> Result<(Image, Artifact), StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |e: String| StepError::body(NAME, e);
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
    let key = KeyBuilder::new(NAME, 2)
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
            let uuid = uuid_of(&object.digest);
            // The tree is appended as it is written. hash_tree reads every
            // data block before it writes the first tree byte, so the
            // reader never sees the tree.
            let mut reader = BufReader::new(
                std::fs::File::open(&data).map_err(|e| fail(format!("read data: {e}")))?,
            );
            let appended = std::fs::OpenOptions::new()
                .append(true)
                .open(&data)
                .map_err(|e| fail(format!("append tree: {e}")))?;
            let mut writer = BufWriter::new(appended);
            let (root, tree_bytes) = hash_tree(
                &mut reader,
                data_blocks,
                object.digest.as_bytes(),
                &mut writer,
            )
            .map_err(fail)?;
            writer
                .flush()
                .map_err(|e| fail(format!("append tree: {e}")))?;
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
                root_hash: tree::hex(&root),
                salt: object.digest.hex(),
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
    /// The appended tree's length, as the hashing wrote it.
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

/// The length rounded up to whole data blocks: the tree covers exactly these
/// bytes, so the image is zero-padded to them.
pub fn padded_len(len: u64, block: u64) -> u64 {
    len.div_ceil(block) * block
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
        // The table's text is read by other programs: its keys, their
        // order and what holds between them are an interface.
        let text = table.text();
        let lines: Vec<(&str, &str)> = text
            .lines()
            .map(|line| line.split_once(' ').expect("key value"))
            .collect();
        let keys: Vec<&str> = lines.iter().map(|(key, _)| *key).collect();
        assert_eq!(
            keys,
            [
                "hash_type",
                "algorithm",
                "root_hash",
                "salt",
                "uuid",
                "data_blocks",
                "data_block_size",
                "hash_block_size",
                "hash_offset",
                "tree_bytes",
                "image_bytes",
                "hash_start_block",
                "table"
            ]
        );
        let value = |key: &str| lines.iter().find(|(k, _)| *k == key).expect(key).1;
        let number = |key: &str| value(key).parse::<u64>().expect(key);
        assert_eq!(value("hash_type"), "1");
        assert_eq!(value("algorithm"), "sha256");
        assert_eq!(value("root_hash").len(), 64);
        assert_eq!(value("salt").len(), 64);
        assert_eq!(
            number("hash_offset"),
            number("data_blocks") * number("data_block_size")
        );
        assert_eq!(
            number("image_bytes"),
            number("hash_offset") + number("tree_bytes")
        );
        assert_eq!(number("hash_block_size"), 4096);
        assert_eq!(
            number("hash_start_block"),
            number("hash_offset") / number("hash_block_size")
        );
        assert!(table.text().contains("hash_start_block 3\n"));
        assert!(
            table
                .text()
                .contains("tree_bytes 4096\nimage_bytes 16384\n")
        );
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
