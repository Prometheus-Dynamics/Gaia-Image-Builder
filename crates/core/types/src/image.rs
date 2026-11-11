//! Images, partitions and sizes.

use crate::StoreRef;
use crate::kind::open_kind;
use daedalus::prelude::Type;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A size in bytes.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Type, Serialize, Deserialize,
)]
#[type_key("gaia:size")]
#[serde(transparent)]
pub struct Size(pub u64);

impl Size {
    pub const fn bytes(bytes: u64) -> Self {
        Self(bytes)
    }

    pub const fn kib(kib: u64) -> Self {
        Self(kib * 1024)
    }

    pub const fn mib(mib: u64) -> Self {
        Self(mib * 1024 * 1024)
    }
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const MIB: u64 = 1024 * 1024;
        match self.0 {
            bytes if bytes >= MIB && bytes % MIB == 0 => write!(f, "{} MiB", bytes / MIB),
            bytes if bytes >= 1024 && bytes % 1024 == 0 => write!(f, "{} KiB", bytes / 1024),
            bytes => write!(f, "{bytes} B"),
        }
    }
}

open_kind!(
    /// What an image is.
    ImageKind, "gaia:image-kind" {
        Disk => "disk",
        Filesystem => "filesystem",
        Slot => "slot",
        Initramfs => "initramfs",
        Archive => "archive",
        Firmware => "firmware",
    }
);

open_kind!(
    /// What a partition holds.
    PartitionRole, "gaia:partition-role" {
        Bootloader => "bootloader",
        Boot => "boot",
        Esp => "esp",
        Rootfs => "rootfs",
        Data => "data",
        Raw => "raw",
    }
);

/// One partition of an image.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:partition")]
pub struct Partition {
    pub name: String,
    pub role: PartitionRole,
    /// The partition table's type for it: a GPT type GUID or an MBR type
    /// byte in hex.
    pub table_type: String,
    pub start: Size,
    pub size: Size,
    /// What is written into it, when anything is.
    pub content: Option<StoreRef>,
}

/// An image: what it is, where it is stored, its size and its partitions
/// (none for anything that is not a disk).
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:image")]
pub struct Image {
    pub kind: ImageKind,
    pub object: StoreRef,
    pub size: Size,
    pub partitions: Vec<Partition>,
}
