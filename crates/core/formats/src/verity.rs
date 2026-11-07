//! dm-verity hash trees as Gaia's `verity` transforms write them with
//! `veritysetup format`: the veritysetup (cryptsetup) version 1 superblock,
//! the layout of an appended tree, and Gaia's derivation of the salt and
//! UUID from the data.
//!
//! The superblock is 512 bytes at the hash offset (cryptsetup
//! `lib/verity/verity.c`, `struct verity_sb`), its integers little-endian:
//!
//! | offset | size | field |
//! |---:|---:|---|
//! | 0 | 8 | signature `"verity\0\0"` |
//! | 8 | 4 | version, 1 |
//! | 12 | 4 | hash type (1: the kernel's format; 0: Chrome OS) |
//! | 16 | 16 | UUID |
//! | 32 | 32 | hash algorithm, NUL-padded (`"sha256"`) |
//! | 64 | 4 | data block size |
//! | 68 | 4 | hash block size |
//! | 72 | 8 | data blocks |
//! | 80 | 2 | salt size |
//! | 82 | 6 | padding |
//! | 88 | 256 | salt, zero-padded |
//! | 344 | 168 | padding |
//!
//! The tree's first hash block follows the superblock, one hash block on.

use sha2::{Digest, Sha256};
use std::fmt;

/// The superblock's bytes.
pub const SUPERBLOCK_SIZE: usize = 512;
/// The superblock's first 8 bytes.
pub const SIGNATURE: [u8; 8] = *b"verity\0\0";
/// The only superblock version veritysetup writes.
pub const SUPERBLOCK_VERSION: u32 = 1;
/// The hash type Gaia writes: the kernel's (`--format=1`).
pub const HASH_TYPE: u32 = 1;
/// The hash algorithm Gaia writes.
pub const ALGORITHM: &str = "sha256";
/// The most salt bytes the superblock holds.
pub const MAX_SALT_SIZE: usize = 256;
/// Gaia's default data and hash block sizes.
pub const DEFAULT_DATA_BLOCK_SIZE: u32 = 4096;
pub const DEFAULT_HASH_BLOCK_SIZE: u32 = 4096;
/// What Gaia hashes ahead of the data's sha256 for the default UUID.
pub const UUID_DOMAIN: &[u8] = b"gaia dm-verity uuid\n";

const ALGORITHM_FIELD: usize = 32;

/// A veritysetup version 1 superblock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Superblock {
    pub hash_type: u32,
    pub uuid: [u8; 16],
    pub algorithm: String,
    pub data_block_size: u32,
    pub hash_block_size: u32,
    pub data_blocks: u64,
    pub salt: Vec<u8>,
}

/// Why 512 bytes are not a superblock, or a superblock cannot be encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuperblockError {
    TooShort(usize),
    Signature([u8; 8]),
    Version(u32),
    Algorithm,
    SaltSize(usize),
    BlockSize { field: &'static str, value: u32 },
}

impl fmt::Display for SuperblockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort(len) => write!(f, "verity superblock cut short at {len} bytes"),
            Self::Signature(found) => write!(f, "verity superblock signature {found:02x?}"),
            Self::Version(version) => write!(f, "verity superblock version {version}"),
            Self::Algorithm => f.write_str(
                "verity superblock hash algorithm is empty, not ASCII or longer than 31 bytes",
            ),
            Self::SaltSize(size) => {
                write!(f, "verity salt of {size} bytes, more than {MAX_SALT_SIZE}")
            }
            Self::BlockSize { field, value } => write!(
                f,
                "verity {field} {value} is not a power of two from 512 to 512 KiB"
            ),
        }
    }
}

impl std::error::Error for SuperblockError {}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn block_size(field: &'static str, value: u32) -> Result<(), SuperblockError> {
    // veritysetup's limits (VERITY_MAX_BLOCK_SIZE).
    if value.is_power_of_two() && (512..=512 * 1024).contains(&value) {
        Ok(())
    } else {
        Err(SuperblockError::BlockSize { field, value })
    }
}

impl Superblock {
    /// The superblock Gaia's `veritysetup format` writes for `data_blocks`
    /// blocks with `salt` and `uuid`.
    pub fn gaia(
        uuid: [u8; 16],
        salt: Vec<u8>,
        data_blocks: u64,
        data_block_size: u32,
        hash_block_size: u32,
    ) -> Self {
        Self {
            hash_type: HASH_TYPE,
            uuid,
            algorithm: ALGORITHM.to_string(),
            data_block_size,
            hash_block_size,
            data_blocks,
            salt,
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, SuperblockError> {
        if bytes.len() < SUPERBLOCK_SIZE {
            return Err(SuperblockError::TooShort(bytes.len()));
        }
        let mut signature = [0u8; 8];
        signature.copy_from_slice(&bytes[..8]);
        if signature != SIGNATURE {
            return Err(SuperblockError::Signature(signature));
        }
        let version = le32(bytes, 8);
        if version != SUPERBLOCK_VERSION {
            return Err(SuperblockError::Version(version));
        }
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&bytes[16..32]);
        let field = &bytes[32..32 + ALGORITHM_FIELD];
        let end = field
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(SuperblockError::Algorithm)?;
        if end == 0 || !field[..end].is_ascii() {
            return Err(SuperblockError::Algorithm);
        }
        let algorithm = String::from_utf8_lossy(&field[..end]).into_owned();
        let salt_size = usize::from(u16::from_le_bytes([bytes[80], bytes[81]]));
        if salt_size > MAX_SALT_SIZE {
            return Err(SuperblockError::SaltSize(salt_size));
        }
        let superblock = Self {
            hash_type: le32(bytes, 12),
            uuid,
            algorithm,
            data_block_size: le32(bytes, 64),
            hash_block_size: le32(bytes, 68),
            data_blocks: u64::from_le_bytes(bytes[72..80].try_into().expect("8 bytes")),
            salt: bytes[88..88 + salt_size].to_vec(),
        };
        block_size("data block size", superblock.data_block_size)?;
        block_size("hash block size", superblock.hash_block_size)?;
        Ok(superblock)
    }

    pub fn encode(&self) -> Result<[u8; SUPERBLOCK_SIZE], SuperblockError> {
        if self.algorithm.is_empty()
            || self.algorithm.len() >= ALGORITHM_FIELD
            || !self.algorithm.is_ascii()
            || self.algorithm.contains('\0')
        {
            return Err(SuperblockError::Algorithm);
        }
        if self.salt.len() > MAX_SALT_SIZE {
            return Err(SuperblockError::SaltSize(self.salt.len()));
        }
        block_size("data block size", self.data_block_size)?;
        block_size("hash block size", self.hash_block_size)?;
        let mut bytes = [0u8; SUPERBLOCK_SIZE];
        bytes[..8].copy_from_slice(&SIGNATURE);
        bytes[8..12].copy_from_slice(&SUPERBLOCK_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.hash_type.to_le_bytes());
        bytes[16..32].copy_from_slice(&self.uuid);
        bytes[32..32 + self.algorithm.len()].copy_from_slice(self.algorithm.as_bytes());
        bytes[64..68].copy_from_slice(&self.data_block_size.to_le_bytes());
        bytes[68..72].copy_from_slice(&self.hash_block_size.to_le_bytes());
        bytes[72..80].copy_from_slice(&self.data_blocks.to_le_bytes());
        bytes[80..82].copy_from_slice(&(self.salt.len() as u16).to_le_bytes());
        bytes[88..88 + self.salt.len()].copy_from_slice(&self.salt);
        Ok(bytes)
    }

    /// The UUID as veritysetup prints it: 8-4-4-4-12 lowercase hex.
    pub fn uuid_string(&self) -> String {
        format_uuid(&self.uuid)
    }

    /// The salt as lowercase hex, as Gaia records it.
    pub fn salt_hex(&self) -> String {
        hex(&self.salt)
    }

    /// The data's length in 512-byte sectors, a dm-verity table's length.
    pub fn data_sectors(&self) -> u64 {
        data_sectors(self.data_blocks, self.data_block_size)
    }
}

/// The values Gaia records for a verity tree (`${assembly.verity:...}`, the
/// assembly state), to compare against the superblock on the media.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    /// Lowercase hex.
    pub salt: String,
    /// 8-4-4-4-12 hex.
    pub uuid: String,
    pub data_blocks: u64,
    pub data_block_size: u32,
    pub hash_block_size: u32,
}

/// One field where the superblock and the record differ, each value as
/// text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatch {
    pub field: &'static str,
    pub superblock: String,
    pub recorded: String,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "verity {}: superblock {}, recorded {}",
            self.field, self.superblock, self.recorded
        )
    }
}

/// Every field where `superblock` differs from `recorded`, plus a hash type
/// or algorithm other than the ones Gaia writes. Empty when they agree.
/// Hex and UUIDs compare without regard to case.
pub fn check(superblock: &Superblock, recorded: &Recorded) -> Vec<Mismatch> {
    let mut mismatches = Vec::new();
    let mut compare = |field: &'static str, found: String, expected: String| {
        if !found.eq_ignore_ascii_case(&expected) {
            mismatches.push(Mismatch {
                field,
                superblock: found,
                recorded: expected,
            });
        }
    };
    compare(
        "hash type",
        superblock.hash_type.to_string(),
        HASH_TYPE.to_string(),
    );
    compare(
        "algorithm",
        superblock.algorithm.clone(),
        ALGORITHM.to_string(),
    );
    compare("salt", superblock.salt_hex(), recorded.salt.clone());
    compare("uuid", superblock.uuid_string(), recorded.uuid.clone());
    compare(
        "data_blocks",
        superblock.data_blocks.to_string(),
        recorded.data_blocks.to_string(),
    );
    compare(
        "data_block_size",
        superblock.data_block_size.to_string(),
        recorded.data_block_size.to_string(),
    );
    compare(
        "hash_block_size",
        superblock.hash_block_size.to_string(),
        recorded.hash_block_size.to_string(),
    );
    mismatches
}

/// Gaia's default salt and UUID for a data image, derived from its sha256
/// so the same data gives the same tree and root hash:
///
/// - salt: the sha256 itself, lowercase hex (32 bytes);
/// - UUID: [`derived_uuid`] of [`UUID_DOMAIN`] and the sha256's lowercase
///   hex text.
///
/// A transform's own `salt` or `uuid` replaces the derived one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Derived {
    pub salt: String,
    pub uuid: String,
}

impl Derived {
    /// From the data's sha256 as hex (case does not matter).
    pub fn from_sha256_hex(sha256_hex: &str) -> Self {
        let sha256_hex = sha256_hex.to_ascii_lowercase();
        Self {
            uuid: derived_uuid(&[UUID_DOMAIN, sha256_hex.as_bytes()]),
            salt: sha256_hex,
        }
    }

    /// From the data's bytes.
    pub fn from_data(data: &[u8]) -> Self {
        Self::from_sha256_hex(&hex(&Sha256::digest(data)))
    }
}

/// A UUID (version 4 layout) derived from `parts`: the first 16 bytes of
/// their sha256, with the version and variant bits set. Gaia also derives
/// ext4 and EROFS UUIDs this way, with their own domains.
pub fn derived_uuid(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format_uuid(&bytes)
}

/// `bytes` as 8-4-4-4-12 lowercase hex.
pub fn format_uuid(bytes: &[u8; 16]) -> String {
    let hex = hex(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// An 8-4-4-4-12 (or bare 32-digit) hex UUID's bytes.
pub fn parse_uuid(text: &str) -> Option<[u8; 16]> {
    let digits: Vec<u8> = text.bytes().filter(|byte| *byte != b'-').collect();
    if digits.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (index, pair) in digits.as_chunks::<2>().0.iter().enumerate() {
        let text = std::str::from_utf8(pair).ok()?;
        bytes[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The data length rounded up to whole data blocks; `veritysetup` covers
/// exactly these bytes, so Gaia zero-pads the image to them.
pub fn padded_len(len: u64, data_block_size: u64) -> u64 {
    len.div_ceil(data_block_size) * data_block_size
}

/// Where an appended tree's superblock goes: the first hash-block boundary
/// at or after the padded data. A tree in its own file has hash offset 0.
pub fn appended_hash_offset(padded_len: u64, hash_block_size: u64) -> u64 {
    padded_len.div_ceil(hash_block_size) * hash_block_size
}

/// The data's length in 512-byte sectors, a dm-verity table's length.
pub fn data_sectors(data_blocks: u64, data_block_size: u32) -> u64 {
    data_blocks * u64::from(data_block_size) / 512
}

/// The tree's first hash block on the hash device, past the superblock
/// (one hash block at `hash_offset`): a dm-verity table's `hash_start`.
pub fn hash_start_block(hash_offset: u64, hash_block_size: u32) -> u64 {
    hash_offset / u64::from(hash_block_size) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_helpers() {
        assert_eq!(padded_len(1, 4096), 4096);
        assert_eq!(padded_len(4097, 4096), 8192);
        assert_eq!(appended_hash_offset(8192, 512), 8192);
        assert_eq!(appended_hash_offset(12288, 8192), 16384);
        assert_eq!(data_sectors(3, 4096), 24);
        assert_eq!(hash_start_block(12288, 4096), 4);
        assert_eq!(hash_start_block(0, 4096), 1);
    }

    #[test]
    fn uuids_parse_and_format() {
        let uuid = derived_uuid(&[b"domain", b"data"]);
        assert_eq!(uuid, derived_uuid(&[b"domain", b"data"]));
        assert_ne!(uuid, derived_uuid(&[b"domain", b"other"]));
        assert_eq!(uuid.len(), 36);
        assert_eq!(uuid.as_bytes()[14], b'4');
        assert!(matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert_eq!(format_uuid(&parse_uuid(&uuid).unwrap()), uuid);
        assert_eq!(parse_uuid("not-a-uuid"), None);
    }
}
