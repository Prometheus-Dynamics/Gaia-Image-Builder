//! `gaia.verity`: a filesystem image with a dm-verity hash tree appended.
//!
//! The image is copied, zero-padded to whole 4 KiB data blocks, and
//! `veritysetup format` writes the tree after it: the superblock at the
//! first hash-block boundary past the data, the tree in the blocks after
//! that. The salt is the image's digest in hex and the UUID is derived from
//! the same digest, so the same image gives the same root hash on every
//! host. The hash type, algorithm and block sizes are fixed (kernel format,
//! sha256, 4 KiB).
//!
//! Two outputs. `image` is the padded data with the tree appended, the same
//! file the device holds. `table` is a text artifact with the root hash,
//! salt, UUID, offsets and the dm-verity table line for `device`, which is
//! the device the image is written to (the tree is on that same device).
//!
//! `veritysetup` is pinned by path; its version is in the key, with the
//! image's digest, so a changed image or tool remakes the tree.

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
/// The superblock and the tree's first block.
const SUPERBLOCK_BLOCKS: u64 = 1;
/// The table file's name in the output directory.
const TABLE_FILE: &str = "verity.txt";
/// The image file's name in the output directory.
const IMAGE_FILE: &str = "image.img";

/// The image with a hash tree appended, and the artifact that says how to open it.
#[node(
    id = "gaia.verity",
    inputs(store, veritysetup, image, device),
    outputs(image, table)
)]
pub fn verity(
    store: &str,
    veritysetup: &str,
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
    let version = std::process::Command::new(veritysetup)
        .arg("--version")
        .env_clear()
        .output()
        .map_err(|e| fail(format!("{veritysetup} --version: {e}")))?;
    let version = [version.stdout, version.stderr].concat();
    let key = KeyBuilder::new(NAME, 1)
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
            let hash_offset = appended_hash_offset(padded, BLOCK);
            let salt = object.digest.hex();
            let uuid = uuid_of(&object.digest);
            let out = work.run(
                &Tool::new(veritysetup)
                    .arg("format")
                    .arg(&data)
                    .arg(&data)
                    .arg("--hash=sha256")
                    .arg(format!("--data-block-size={BLOCK}"))
                    .arg(format!("--hash-block-size={BLOCK}"))
                    .arg(format!("--salt={salt}"))
                    .arg(format!("--uuid={uuid}"))
                    .arg(format!("--hash-offset={hash_offset}")),
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
            if blocks != padded / BLOCK {
                return Err(fail(format!(
                    "veritysetup covered {blocks} data blocks, expected {}",
                    padded / BLOCK
                )));
            }
            let table = Table {
                root_hash,
                salt,
                uuid,
                data_blocks: blocks,
                hash_offset,
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
    pub hash_offset: u64,
    pub device: String,
}

impl Table {
    /// The data's length in 512-byte sectors.
    pub fn sectors(&self) -> u64 {
        self.data_blocks * BLOCK / 512
    }

    /// The tree's first hash block on the device, past the superblock.
    pub fn hash_start(&self) -> u64 {
        self.hash_offset / BLOCK + SUPERBLOCK_BLOCKS
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
             hash_offset {}\nhash_start_block {}\ntable {}\n",
            self.root_hash,
            self.salt,
            self.uuid,
            self.data_blocks,
            self.hash_offset,
            self.hash_start(),
            self.line(),
        )
    }
}

/// The length rounded up to whole data blocks: `veritysetup` covers exactly
/// these bytes, so the image is zero-padded to them.
pub fn padded_len(len: u64, block: u64) -> u64 {
    len.div_ceil(block) * block
}

/// Where the appended tree's superblock goes: the first hash-block boundary
/// at or after the padded data.
pub fn appended_hash_offset(padded: u64, block: u64) -> u64 {
    padded.div_ceil(block) * block
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

fn is_hex_of_len(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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
    fn layout_pads_to_blocks_and_puts_the_superblock_after_the_data() {
        assert_eq!(padded_len(1, BLOCK), BLOCK);
        assert_eq!(padded_len(BLOCK, BLOCK), BLOCK);
        assert_eq!(padded_len(BLOCK + 1, BLOCK), 2 * BLOCK);
        assert_eq!(appended_hash_offset(3 * BLOCK, BLOCK), 3 * BLOCK);
    }

    #[test]
    fn table_line_is_the_dm_verity_form() {
        let table = Table {
            root_hash: "ab".repeat(32),
            salt: "cd".repeat(32),
            uuid: "01234567-89ab-4cde-8f01-23456789abcd".into(),
            data_blocks: 3,
            hash_offset: 3 * BLOCK,
            device: "/dev/vda3".into(),
        };
        assert_eq!(table.sectors(), 24);
        assert_eq!(table.hash_start(), 4);
        assert_eq!(
            table.line(),
            format!(
                "0 24 verity 1 /dev/vda3 /dev/vda3 4096 4096 3 4 sha256 {} {}",
                "ab".repeat(32),
                "cd".repeat(32)
            )
        );
        assert!(table.text().contains("hash_start_block 4\n"));
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
    fn uuids_are_version_4() {
        let uuid = uuid_of(&Digest::of_bytes(b"image"));
        assert_eq!(uuid.len(), 36);
        assert_eq!(uuid.as_bytes()[14], b'4');
        assert!(matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert_eq!(uuid, uuid_of(&Digest::of_bytes(b"image")));
    }
}
