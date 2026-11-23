//! What an erofs image says it was made with, read from its superblock.
//!
//! The superblock is 144 bytes at offset 1024 (`erofs_fs.h`, erofs-utils and
//! the kernel agree): `feature_compat` at 8, `blkszbits` at 12 (the block
//! size is `1 << blkszbits`), `feature_incompat` at 80, the
//! two-byte `u1` at 84 (the `lz4_max_distance`, or with `compr_cfgs` the
//! bitmap of compression algorithms), and `packed_nid` at 96. The bits were
//! checked against `dump.erofs -s` on images made with `mkfs.erofs`:
//!
//! - no compression: `u1` is 0, and neither `lz4_0padding` nor `compr_cfgs`;
//! - lz4 and lz4hc: `lz4_0padding`, and with no cluster size (`-C`) no
//!   `compr_cfgs`, `u1` the distance. With a cluster size, big pclusters:
//!   `compr_cfgs` too, and `u1` is the bitmap 0x1 (mkfs.erofs 1.9.4,
//!   `-zlz4hc -C1048576`: incompat 0x13, u1 0x1). The superblock does not
//!   tell lz4 and lz4hc apart;
//! - lzma, deflate, zstd: `compr_cfgs`, and `u1` is exactly one bit: 0x2,
//!   0x4, 0x8 (the algorithm's index in the kernel's `Z_EROFS_COMPRESSION_*`,
//!   lz4's being 0x1).
//! - `fragments` is incompat 0x20 with a packed inode, whose nid is
//!   `packed_nid`; `dedupe` shares the bit and makes no packed inode, so
//!   `packed_nid` is what tells fragments apart.
//! - `ztailpacking` is incompat 0x10, set only when some file's tail was
//!   packed. A file whose tail does not pack (random data, say) leaves the
//!   bit clear, and so does a tree with no tails. So a clear bit is not
//!   evidence that the option was ignored.

/// The magic number at the superblock.
pub const MAGIC: u32 = 0xE0F5_E1E2;
/// The block size every image is made with (`mkfs.erofs -b`), and the
/// superblock must say.
pub const BLOCK_SIZE: u32 = 4096;
/// Where the superblock starts in an image.
const OFFSET: usize = 1024;
const FEATURE_LZ4_0PADDING: u32 = 0x1;
const FEATURE_COMPR_CFGS: u32 = 0x2;
const FEATURE_ZTAILPACKING: u32 = 0x10;
const FEATURE_FRAGMENTS: u32 = 0x20;

/// The superblock fields the checks read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superblock {
    pub feature_compat: u32,
    /// The block size's power of two.
    pub blkszbits: u8,
    pub feature_incompat: u32,
    pub u1: u16,
    pub packed_nid: u64,
}

/// What an image says of its own compression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compressor {
    None,
    /// lz4 or lz4hc: the superblock does not tell which.
    Lz4,
    Lzma,
    Deflate,
    Zstd,
}

/// The superblock of `bytes`, an image's first bytes.
///
/// # Errors
///
/// Too short for a superblock, or no erofs magic number at its place.
pub fn read(bytes: &[u8]) -> Result<Superblock, String> {
    let at = |offset: usize, len: usize| -> Result<&[u8], String> {
        bytes
            .get(OFFSET + offset..OFFSET + offset + len)
            .ok_or_else(|| {
                format!(
                    "the image is {} bytes: too short for its superblock",
                    bytes.len()
                )
            })
    };
    let magic = u32::from_le_bytes(at(0, 4)?.try_into().unwrap_or([0; 4]));
    if magic != MAGIC {
        return Err(format!("no erofs magic number at {OFFSET}: {magic:#010x}"));
    }
    let word = |offset: usize| -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            at(offset, 4)?.try_into().unwrap_or([0; 4]),
        ))
    };
    let u1 = u16::from_le_bytes(at(84, 2)?.try_into().unwrap_or([0; 2]));
    let packed_nid = u64::from_le_bytes(at(96, 8)?.try_into().unwrap_or([0; 8]));
    Ok(Superblock {
        feature_compat: word(8)?,
        blkszbits: at(12, 1)?[0],
        feature_incompat: word(80)?,
        u1,
        packed_nid,
    })
}

/// The compressor the superblock says the image was written with, or why it
/// names none.
///
/// # Errors
///
/// A bitmap of algorithms that is none of the four, or more than one.
pub fn compressor(sb: &Superblock) -> Result<Compressor, String> {
    if sb.feature_incompat & FEATURE_COMPR_CFGS == 0 {
        return if sb.feature_incompat & FEATURE_LZ4_0PADDING != 0 {
            Ok(Compressor::Lz4)
        } else if sb.u1 == 0 {
            Ok(Compressor::None)
        } else {
            Err(format!("no compression feature, and u1 is {:#x}", sb.u1))
        };
    }
    match sb.u1 {
        0x1 => Ok(Compressor::Lz4),
        0x2 => Ok(Compressor::Lzma),
        0x4 => Ok(Compressor::Deflate),
        0x8 => Ok(Compressor::Zstd),
        other => Err(format!(
            "the algorithms are {other:#x}: not one of lz4, lzma, deflate, zstd"
        )),
    }
}

/// Checks the image's superblock against what was asked: the block size
/// ([`BLOCK_SIZE`]), the compression (`none`, lz4 or lz4hc, lzma, deflate,
/// zstd), `fragments` and `ztailpacking`. Returns one line of what the image
/// holds.
///
/// # Errors
///
/// A compressor, fragments or ztailpacking that the image does not hold as
/// asked, named, so a tool that ignores an option fails the step.
pub fn check(sb: &Superblock, compression: &str, extended: &[&str]) -> Result<String, String> {
    let block = 1u64.checked_shl(u32::from(sb.blkszbits)).unwrap_or(0);
    if block != u64::from(BLOCK_SIZE) {
        return Err(format!(
            "the image's blocks are {block} bytes (blkszbits {}), and {BLOCK_SIZE} was asked: the tool did not write it",
            sb.blkszbits
        ));
    }
    let got = compressor(sb)?;
    let (want, label) = match compression {
        "none" => (Compressor::None, "none"),
        "lz4" | "lz4hc" => (Compressor::Lz4, "lz4"),
        "lzma" => (Compressor::Lzma, "lzma"),
        "deflate" => (Compressor::Deflate, "deflate"),
        "zstd" => (Compressor::Zstd, "zstd"),
        other => return Err(format!("no superblock rule for compression {other:?}")),
    };
    if got != want {
        return Err(format!(
            "the image says it was written with {got:?}, and {label} was asked: the tool did not write it"
        ));
    }
    let fragments = extended.contains(&"fragments");
    let ztail = extended.contains(&"ztailpacking");
    let packed = sb.packed_nid != 0;
    if fragments != packed {
        return Err(if fragments {
            "fragments were asked, and the image has no packed inode".into()
        } else {
            "fragments were not asked, and the image has a packed inode".into()
        });
    }
    if fragments && sb.feature_incompat & FEATURE_FRAGMENTS == 0 {
        return Err("fragments were asked, and the image has no fragments feature".into());
    }
    let ztail_set = sb.feature_incompat & FEATURE_ZTAILPACKING != 0;
    if ztail_set && !ztail {
        return Err("ztailpacking was not asked, and the image has it".into());
    }
    Ok(format!(
        "superblock: compression {label}, fragments {}, ztailpacking {}",
        if packed { "yes" } else { "no" },
        if ztail_set { "yes" } else { "no" },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A superblock image of the given fields, as bytes.
    fn image(incompat: u32, u1: u16, packed_nid: u64) -> Vec<u8> {
        let mut b = vec![0u8; OFFSET + 144];
        b[OFFSET..OFFSET + 4].copy_from_slice(&MAGIC.to_le_bytes());
        b[OFFSET + 8..OFFSET + 12].copy_from_slice(&3u32.to_le_bytes());
        b[OFFSET + 12] = 12;
        b[OFFSET + 80..OFFSET + 84].copy_from_slice(&incompat.to_le_bytes());
        b[OFFSET + 84..OFFSET + 86].copy_from_slice(&u1.to_le_bytes());
        b[OFFSET + 96..OFFSET + 104].copy_from_slice(&packed_nid.to_le_bytes());
        b
    }

    fn sb(incompat: u32, u1: u16, packed_nid: u64) -> Superblock {
        read(&image(incompat, u1, packed_nid)).expect("read")
    }

    /// Each compressor's bits read as the superblock names them.
    #[test]
    fn each_compressor_reads_as_its_bits() {
        assert_eq!(compressor(&sb(0, 0, 0)), Ok(Compressor::None));
        assert_eq!(compressor(&sb(0x1, 65535, 0)), Ok(Compressor::Lz4));
        // lz4hc with a cluster size: big pclusters set compr_cfgs, and the
        // bitmap is lz4's bit (mkfs.erofs 1.9.4 -zlz4hc -C1048576).
        assert_eq!(compressor(&sb(0x13, 0x1, 0)), Ok(Compressor::Lz4));
        assert_eq!(compressor(&sb(0x3, 0x2, 0)), Ok(Compressor::Lzma));
        assert_eq!(compressor(&sb(0x3, 0x4, 0)), Ok(Compressor::Deflate));
        assert_eq!(compressor(&sb(0x3, 0x8, 0)), Ok(Compressor::Zstd));
        assert!(compressor(&sb(0x3, 0x3, 0)).is_err(), "two algorithms");
    }

    /// The block size must be the one asked: 4096 reads back, a 16 KiB
    /// image (a 16 KiB-page host's default) is refused.
    #[test]
    fn the_block_size_must_be_4096() {
        let mut b = image(0, 0, 0);
        assert_eq!(read(&b).expect("read").blkszbits, 12);
        assert!(check(&read(&b).expect("read"), "none", &[]).is_ok());
        b[OFFSET + 12] = 14;
        let why = check(&read(&b).expect("read"), "none", &[]).expect_err("16 KiB");
        assert!(
            why.contains("the image's blocks are 16384 bytes (blkszbits 14), and 4096 was asked"),
            "{why}"
        );
    }

    /// The image must be an erofs one, and long enough.
    #[test]
    fn a_non_erofs_or_short_file_is_refused() {
        let mut bad = image(0, 0, 0);
        bad[OFFSET] = 0;
        assert!(
            read(&bad)
                .expect_err("an error")
                .contains("no erofs magic number")
        );
        assert!(
            read(&bad[..OFFSET])
                .expect_err("an error")
                .contains("too short")
        );
    }

    /// The compressor asked for must be the one written; lz4 and lz4hc are
    /// one family.
    #[test]
    fn the_compressor_asked_must_be_the_one_written() {
        let lzma = sb(0x3, 0x2, 0);
        assert!(check(&lzma, "lzma", &[]).is_ok());
        assert!(
            check(&lzma, "zstd", &[])
                .expect_err("an error")
                .contains("zstd was asked")
        );
        let lz4hc = sb(0x1, 65535, 0);
        assert!(check(&lz4hc, "lz4hc", &[]).is_ok());
        assert!(check(&lz4hc, "lz4", &[]).is_ok());
        assert!(check(&lz4hc, "none", &[]).is_err());
        let big = sb(0x3, 0x1, 0);
        assert!(check(&big, "lz4hc", &[]).is_ok());
        assert!(check(&big, "lzma", &[]).is_err());
    }

    /// Fragments are asked as the packed inode: the image has one exactly
    /// when fragments were asked.
    #[test]
    fn fragments_are_the_packed_inode() {
        let frag = sb(0x21, 65535, 60);
        assert!(check(&frag, "lz4hc", &["fragments"]).is_ok());
        assert!(
            check(&frag, "lz4hc", &[])
                .expect_err("an error")
                .contains("not asked")
        );
        let plain = sb(0x1, 65535, 0);
        assert!(check(&plain, "lz4hc", &[]).is_ok());
        assert!(
            check(&plain, "lz4hc", &["fragments"])
                .expect_err("an error")
                .contains("no packed inode")
        );
    }

    /// Ztailpacking: set when not asked is refused. Clear when asked is
    /// accepted, since a tail that does not pack leaves the bit clear.
    #[test]
    fn ztailpacking_set_only_when_asked() {
        let ztail = sb(0x11, 65535, 0);
        assert!(check(&ztail, "lz4hc", &["ztailpacking"]).is_ok());
        assert!(
            check(&ztail, "lz4hc", &[])
                .expect_err("an error")
                .contains("not asked")
        );
        let clear = sb(0x1, 65535, 0);
        let line = check(&clear, "lz4hc", &["ztailpacking"]).expect("accepted");
        assert!(line.ends_with("ztailpacking no"), "{line}");
    }
}
