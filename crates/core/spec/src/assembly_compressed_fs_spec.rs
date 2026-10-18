//! The options of the read-only compressed filesystem kinds, `squashfs` and
//! `erofs`: which compressor each accepts, its levels, and the defaults.

use std::ops::RangeInclusive;

/// Options of a `squashfs` or `erofs` image. Every field is `None` or
/// `false` for the other kinds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssemblyCompressedFsSpec {
    /// The compressor by name (see [`SquashfsCompression`] and
    /// [`ErofsCompression`]); `None` uses the kind's default.
    pub compression: Option<String>,
    /// Block size in bytes; `None` uses the tool default (squashfs 128 KiB,
    /// erofs 4096).
    pub block_size: Option<u32>,
    /// `erofs` only: the largest physical cluster in bytes (`-C`).
    pub pcluster_size: Option<u32>,
    /// `erofs` only: deduplicate compressed data (`-E dedupe`).
    pub dedupe: bool,
    /// `erofs` only: pack file tails into shared fragments (`-E fragments`).
    pub fragments: bool,
    /// `erofs` only: inline the compressed tail of a file in its inode
    /// (`-E ztailpacking`).
    pub ztailpacking: bool,
}

/// The `squashfs` block sizes `mksquashfs -b` accepts: powers of two in
/// this range.
pub const SQUASHFS_BLOCK_SIZES: RangeInclusive<u32> = 4096..=1024 * 1024;
/// The `erofs` block sizes `mkfs.erofs -b` accepts: powers of two in this
/// range (the kernel mounts only those up to its page size).
pub const EROFS_BLOCK_SIZES: RangeInclusive<u32> = 512..=64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SquashfsCompression {
    /// The default: every squashfs kernel reads it.
    Gzip,
    Xz,
    Zstd,
    Lz4,
}

impl SquashfsCompression {
    pub const NAMES: &'static str = "gzip, xz, zstd or lz4";

    pub fn parse(raw: Option<&str>) -> Option<Self> {
        match raw.unwrap_or("gzip") {
            "gzip" => Some(Self::Gzip),
            "xz" => Some(Self::Xz),
            "zstd" => Some(Self::Zstd),
            "lz4" => Some(Self::Lz4),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::Xz => "xz",
            Self::Zstd => "zstd",
            Self::Lz4 => "lz4",
        }
    }

    /// The `-Xcompression-level` values, or `None` when the compressor
    /// takes no level.
    pub fn levels(self) -> Option<RangeInclusive<u32>> {
        match self {
            Self::Gzip => Some(1..=9),
            Self::Zstd => Some(1..=22),
            Self::Xz | Self::Lz4 => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErofsCompression {
    /// Uncompressed.
    None,
    Lz4,
    /// The default: LZ4 high compression, read by the kernel's LZ4 decoder.
    Lz4hc,
    Lzma,
    Deflate,
}

impl ErofsCompression {
    pub const NAMES: &'static str = "none, lz4, lz4hc, lzma or deflate";

    pub fn parse(raw: Option<&str>) -> Option<Self> {
        match raw.unwrap_or("lz4hc") {
            "none" => Some(Self::None),
            "lz4" => Some(Self::Lz4),
            "lz4hc" => Some(Self::Lz4hc),
            "lzma" => Some(Self::Lzma),
            "deflate" => Some(Self::Deflate),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Lz4 => "lz4",
            Self::Lz4hc => "lz4hc",
            Self::Lzma => "lzma",
            Self::Deflate => "deflate",
        }
    }

    /// The levels `-z<name>,<level>` accepts, or `None` when the compressor
    /// takes no level.
    pub fn levels(self) -> Option<RangeInclusive<u32>> {
        match self {
            Self::Lz4hc => Some(0..=12),
            Self::Lzma | Self::Deflate => Some(0..=9),
            Self::None | Self::Lz4 => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressors_parse_their_names_and_default_when_unset() {
        assert_eq!(
            SquashfsCompression::parse(None),
            Some(SquashfsCompression::Gzip)
        );
        assert_eq!(
            SquashfsCompression::parse(Some("zstd")),
            Some(SquashfsCompression::Zstd)
        );
        assert_eq!(SquashfsCompression::parse(Some("lzma")), None);
        assert_eq!(ErofsCompression::parse(None), Some(ErofsCompression::Lz4hc));
        assert_eq!(
            ErofsCompression::parse(Some("none")),
            Some(ErofsCompression::None)
        );
        assert_eq!(ErofsCompression::parse(Some("zstd")), None);
        for compression in [
            SquashfsCompression::Gzip,
            SquashfsCompression::Xz,
            SquashfsCompression::Zstd,
            SquashfsCompression::Lz4,
        ] {
            assert_eq!(
                SquashfsCompression::parse(Some(compression.as_str())),
                Some(compression)
            );
        }
    }
}
