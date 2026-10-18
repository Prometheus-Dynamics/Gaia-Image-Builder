//! The transforms and filesystem images an assembly builds from its trees
//! and files: how each is written, and which options each kind accepts.

use crate::{
    AssemblyCompressedFsSpec, AssemblyFilesystemId, AssemblyPathTemplate, AssemblyTreeId, ByteSize,
    ByteSizeParseError,
};
use std::str::FromStr;

/// The dm-verity data and hash block size when the spec sets none.
pub const VERITY_DEFAULT_BLOCK_SIZE: u32 = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyTransformSpec {
    pub kind: AssemblyTransformKindSpec,
    pub src: Option<AssemblyPathTemplate>,
    pub dest: AssemblyPathTemplate,
    pub deterministic: bool,
    /// Compression level: `zstd` 1-19, `xz` 0-9 (its preset); `None` uses
    /// the tool default.
    pub level: Option<u32>,
    /// `verity` only: the hash tree's own file. `None` appends the tree to
    /// `dest`, after the data.
    pub hash_dest: Option<AssemblyPathTemplate>,
    /// `verity` only: data block size in bytes; `None` is 4096.
    pub data_block_size: Option<u32>,
    /// `verity` only: hash block size in bytes; `None` is 4096.
    pub hash_block_size: Option<u32>,
    /// `verity` only: the salt as hex; `None` derives it from the data
    /// image's sha256, so the same data gives the same tree.
    pub salt: Option<String>,
    /// `verity` only: the superblock UUID; `None` derives it from the data.
    pub uuid: Option<String>,
}

impl AssemblyTransformSpec {
    /// The verity data block size, with the default applied.
    pub fn verity_data_block_size(&self) -> u32 {
        self.data_block_size.unwrap_or(VERITY_DEFAULT_BLOCK_SIZE)
    }

    /// The verity hash block size, with the default applied.
    pub fn verity_hash_block_size(&self) -> u32 {
        self.hash_block_size.unwrap_or(VERITY_DEFAULT_BLOCK_SIZE)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyTransformKindSpec {
    CompileDts,
    Gzip,
    Zstd,
    /// xz, multithreaded and with a fixed block size, so the output is the
    /// same for any thread count.
    Xz,
    Copy,
    /// A dm-verity hash tree over a data image (`src`), written to `dest`
    /// with the tree appended or to `hash_dest`.
    Verity,
}

impl AssemblyTransformKindSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CompileDts => "compile-dts",
            Self::Gzip => "gzip",
            Self::Zstd => "zstd",
            Self::Xz => "xz",
            Self::Copy => "copy",
            Self::Verity => "verity",
        }
    }
}

// Debug is in assembly_spec_debug: fingerprints hash it.
#[derive(Clone, PartialEq, Eq)]
pub struct AssemblyFilesystemSpec {
    pub id: AssemblyFilesystemId,
    pub kind: AssemblyFilesystemKindSpec,
    pub source_tree: AssemblyTreeId,
    pub output: AssemblyPathTemplate,
    /// Byte size, or `"auto"` for vfat and ext4 (sized from the tree).
    pub size: Option<String>,
    pub deterministic: bool,
    /// zstd level for `cpio-zstd` (1-19) or xz level for `cpio-xz` (0-9);
    /// `None` uses the default.
    pub compression_level: Option<u32>,
    /// Also copy the image to `$assembly.out/<file name of output>`, the
    /// image output dir, where it is published as an image of the run.
    pub publish: bool,
    /// `ext4` only: the volume label.
    pub label: Option<String>,
    /// `ext4` only: the filesystem UUID; `None` derives it from the id.
    pub uuid: Option<String>,
    /// `ext4` only: whether to create a journal; `None` is on.
    pub journal: Option<bool>,
    /// Only with `size = "auto"`: free space to leave beyond the content, a
    /// byte size; `None` leaves only the automatic margin.
    pub free: Option<String>,
    /// `squashfs` and `erofs` only: compression, block size and the erofs
    /// layout options.
    pub compressed: AssemblyCompressedFsSpec,
}

impl AssemblyFilesystemSpec {
    pub fn parsed_size(&self) -> Result<Option<ByteSize>, ByteSizeParseError> {
        self.size.as_deref().map(ByteSize::from_str).transpose()
    }

    pub fn parsed_free(&self) -> Result<Option<ByteSize>, ByteSizeParseError> {
        self.free.as_deref().map(ByteSize::from_str).transpose()
    }

    /// Where a published filesystem image is copied: the image output dir
    /// (`$assembly.out`) under the output's file name. `None` when the
    /// filesystem is not published or the output has no file name.
    pub fn publish_template(&self) -> Option<AssemblyPathTemplate> {
        if !self.publish {
            return None;
        }
        let name = std::path::Path::new(self.output.as_str())
            .file_name()?
            .to_str()?;
        Some(AssemblyPathTemplate::new(format!("$assembly.out/{name}")))
    }

    /// Whether an `ext4` image gets a journal (the default).
    pub fn journal_enabled(&self) -> bool {
        self.journal.unwrap_or(true)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyFilesystemKindSpec {
    Vfat,
    Cpio,
    CpioGzip,
    CpioZstd,
    /// A newc cpio archive compressed with xz and a CRC32 check (the kernel's
    /// XZ decoder reads that check).
    CpioXz,
    /// An ext4 image of the tree, built with `mkfs.ext4 -d`.
    Ext4,
    /// A read-only squashfs image of the tree, built with `mksquashfs`.
    Squashfs,
    /// A read-only EROFS image of the tree, built with `mkfs.erofs`.
    Erofs,
}

impl AssemblyFilesystemKindSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vfat => "vfat",
            Self::Cpio => "cpio",
            Self::CpioGzip => "cpio-gzip",
            Self::CpioZstd => "cpio-zstd",
            Self::CpioXz => "cpio-xz",
            Self::Ext4 => "ext4",
            Self::Squashfs => "squashfs",
            Self::Erofs => "erofs",
        }
    }
}
